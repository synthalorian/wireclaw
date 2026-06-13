//! HTTP/HTTPS proxy server that intercepts and forwards requests.
//!
//! For HTTP: direct proxying with full capture.
//! For HTTPS (CONNECT): MITM termination — presents a cert signed by our CA
//! to the client, then makes a separate TLS connection upstream.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;

use crate::cert::CertManager;
use crate::models::{CapturedRequest, CapturedResponse, Exchange};

// BoxBody type alias for convenience
type BoxBody = http_body_util::combinators::BoxBody<Bytes, Infallible>;

pub struct ProxyServer {
    listen_addr: SocketAddr,
    exchange_tx: mpsc::Sender<Exchange>,
    session: String,
    cert_manager: Arc<CertManager>,
    intercept_rules: Option<Vec<crate::intercept::InterceptRule>>,
}

impl ProxyServer {
    pub fn new(
        listen_addr: SocketAddr,
        exchange_tx: mpsc::Sender<Exchange>,
        session: String,
        cert_manager: Arc<CertManager>,
        intercept_rules: Option<Vec<crate::intercept::InterceptRule>>,
    ) -> Self {
        Self {
            listen_addr,
            exchange_tx,
            session,
            cert_manager,
            intercept_rules,
        }
    }

    pub async fn run(self: Arc<Self>) -> Result<()> {
        let listener = TcpListener::bind(self.listen_addr).await?;
        eprintln!("[ledger] proxy listening on {}", self.listen_addr);

        loop {
            let (stream, _remote) = listener.accept().await?;
            let tx = self.exchange_tx.clone();
            let session = self.session.clone();
            let cert_mgr = Arc::clone(&self.cert_manager);
            let intercept_rules = self.intercept_rules.clone();

            tokio::spawn(async move {
                if let Err(e) =
                    handle_connection(stream, tx, session, cert_mgr, intercept_rules).await
                {
                    eprintln!("[ledger] proxy connection error: {e}");
                }
            });
        }
    }
}

/// Handle an incoming TCP connection.
/// First, try to read the initial bytes to detect if it's a CONNECT request.
/// If so, handle tunneling (MITM for HTTPS). Otherwise, pass to hyper for HTTP proxying.
async fn handle_connection(
    client_stream: TcpStream,
    tx: mpsc::Sender<Exchange>,
    session: String,
    cert_mgr: Arc<CertManager>,
    intercept_rules: Option<Vec<crate::intercept::InterceptRule>>,
) -> Result<()> {
    // Peek at the first bytes to detect CONNECT
    let mut peek_buf = [0u8; 8];
    let n = client_stream.peek(&mut peek_buf).await?;
    let starts_with_connect = n >= 7 && &peek_buf[..7] == b"CONNECT";

    if starts_with_connect {
        handle_connect_tunnel(client_stream, tx, session, cert_mgr, intercept_rules).await
    } else {
        handle_http_proxy(client_stream, tx, session, intercept_rules).await
    }
}

/// Handle HTTP proxy requests (non-CONNECT) via hyper.
async fn handle_http_proxy(
    stream: TcpStream,
    tx: mpsc::Sender<Exchange>,
    session: String,
    intercept_rules: Option<Vec<crate::intercept::InterceptRule>>,
) -> Result<()> {
    let io = hyper_util::rt::TokioIo::new(stream);
    let svc = service_fn(move |req| {
        let tx = tx.clone();
        let session = session.clone();
        let rules = intercept_rules.clone();
        async move { handle_request(req, tx, session, rules).await }
    });

    http1::Builder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .serve_connection(io, svc)
        .with_upgrades()
        .await
        .map_err(|e| anyhow::anyhow!("http proxy error: {e}"))?;

    Ok(())
}

/// Handle CONNECT tunneling with MITM TLS termination.
///
/// Flow:
/// 1. Read CONNECT request, parse authority (host:port)
/// 2. Send 200 Connection Established to client
/// 3. Wrap client side in TLS (presenting a per-host cert signed by our CA)
/// 4. Make a separate TLS connection to the upstream
/// 5. For each HTTP request on the client TLS stream:
///    a. Capture the plaintext request
///    b. Forward it over the upstream TLS connection
///    c. Capture the plaintext response
///    d. Return it to the client
async fn handle_connect_tunnel(
    mut client_stream: TcpStream,
    tx: mpsc::Sender<Exchange>,
    session: String,
    cert_mgr: Arc<CertManager>,
    intercept_rules: Option<Vec<crate::intercept::InterceptRule>>,
) -> Result<()> {
    // Read the CONNECT request line and headers
    let mut buf = vec![0u8; 4096];
    let n = client_stream.read(&mut buf).await?;
    buf.truncate(n);

    let request_str = String::from_utf8_lossy(&buf);
    let first_line = request_str.lines().next().unwrap_or("");

    // Parse: CONNECT host:port HTTP/1.1
    let authority = first_line
        .strip_prefix("CONNECT ")
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("")
        .to_string();

    if authority.is_empty() {
        let response = b"HTTP/1.1 400 Bad Request\r\n\r\n";
        client_stream.write_all(response).await?;
        return Ok(());
    }

    // Connect to target (for verifying reachability; we'll reconnect with TLS later)
    let upstream = match TcpStream::connect(&authority).await {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!("[ledger] CONNECT failed to {}: {}", authority, e);
            let response = b"HTTP/1.1 502 Bad Gateway\r\n\r\n";
            client_stream.write_all(response).await?;
            return Ok(());
        }
    };
    // We don't need this plain TCP stream yet; we'll make a proper TLS connection later
    drop(upstream);

    // Send 200 OK to client
    let response = b"HTTP/1.1 200 Connection Established\r\n\r\n";
    client_stream.write_all(response).await?;

    // Now wrap the client side in TLS using a cert for this host
    let host = authority.split(':').next().unwrap_or(&authority);
    let server_config = cert_mgr.server_config_for_host(host)?;
    let acceptor = TlsAcceptor::from(server_config);

    let client_tls = match acceptor.accept(client_stream).await {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!(
                "[ledger] TLS handshake with client failed for {}: {}",
                authority, e
            );
            return Ok(());
        }
    };

    let authority_for_error = authority.clone();

    // Run HTTP proxy logic over the TLS stream
    let io = hyper_util::rt::TokioIo::new(client_tls);
    let svc = service_fn(move |req| {
        let tx = tx.clone();
        let session = session.clone();
        let authority = authority.clone();
        let rules = intercept_rules.clone();
        async move { handle_https_request(req, tx, session, authority, rules).await }
    });

    if let Err(e) = http1::Builder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .serve_connection(io, svc)
        .with_upgrades()
        .await
    {
        eprintln!(
            "[ledger] HTTPS proxy error for {}: {}",
            authority_for_error, e
        );
    }

    Ok(())
}

/// Handle an HTTPS request inside a CONNECT tunnel (after TLS termination).
/// The request is plaintext (we terminated TLS). We need to:
/// 1. Capture it
/// 2. Forward it over a new TLS connection to the upstream
/// 3. Capture the response
/// 4. Return it to the client
async fn handle_https_request(
    req: Request<Incoming>,
    tx: mpsc::Sender<Exchange>,
    session: String,
    authority: String,
    intercept_rules: Option<Vec<crate::intercept::InterceptRule>>,
) -> Result<Response<BoxBody>, Infallible> {
    match proxy_https_request(req, tx, session, authority, intercept_rules).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            eprintln!("[ledger] HTTPS proxy error: {e}");
            let body = Full::new(Bytes::from(format!("ledger proxy error: {e}")))
                .map_err(|never| match never {})
                .boxed();
            Ok(Response::builder()
                .status(502)
                .body(body)
                .expect("BoxBody should always construct successfully"))
        }
    }
}

async fn proxy_https_request(
    mut req: Request<Incoming>,
    tx: mpsc::Sender<Exchange>,
    session: String,
    authority: String,
    intercept_rules: Option<Vec<crate::intercept::InterceptRule>>,
) -> Result<Response<BoxBody>> {
    let start = Instant::now();

    // Check for WebSocket upgrade before consuming req
    let is_ws_upgrade = crate::websocket::is_websocket_upgrade(&req);

    // Register for upgrade BEFORE consuming the request
    let on_upgrade = if is_ws_upgrade {
        Some(hyper::upgrade::on(&mut req))
    } else {
        None
    };

    // Build outgoing request
    let (parts, body) = req.into_parts();

    // Capture request headers BEFORE we consume them
    let mut req_headers = std::collections::HashMap::new();
    for (k, v) in &parts.headers {
        if let Ok(val) = v.to_str() {
            req_headers.insert(k.as_str().to_lowercase(), val.to_string());
        }
    }

    // Reconstruct absolute URI from authority + path
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let uri_str = format!("https://{}{}", authority, path_and_query);
    let uri = uri_str.parse::<Uri>()?;

    let host = uri.host().unwrap_or("unknown").to_string();
    let path = uri.path().to_string();
    let method = parts.method.to_string();

    // Collect request body
    let req_body_bytes = body
        .collect()
        .await
        .map_err(|e| anyhow::anyhow!("failed to read request body: {e}"))?
        .to_bytes();

    let request_id = uuid::Uuid::new_v4().to_string();
    let captured_req = CapturedRequest {
        id: request_id.clone(),
        method: method.clone(),
        url: uri.to_string(),
        path: path.clone(),
        host: host.clone(),
        headers: req_headers,
        body: if req_body_bytes.is_empty() {
            None
        } else {
            Some(req_body_bytes.to_vec())
        },
        timestamp: chrono::Utc::now(),
        session: session.clone(),
    };

    if is_ws_upgrade {
        eprintln!("[ledger] WebSocket upgrade detected: {} {}", method, uri);
    }

    // Check intercept rules in HTTPS path too
    if let Some(ref rules) = intercept_rules {
        let engine = crate::intercept::InterceptEngine::new(rules.clone(), true);
        if engine.should_intercept(&captured_req) {
            let (decision, modified_req) = engine.prompt(&captured_req).await?;
            match decision {
                crate::intercept::InterceptDecision::Drop => {
                    let body = Full::new(Bytes::from("Intercepted and dropped by ledger"))
                        .map_err(|never| match never {})
                        .boxed();
                    return Ok(Response::builder().status(502).body(body)?);
                }
                crate::intercept::InterceptDecision::Modify => {
                    if let Some(ref modified) = modified_req {
                        let mut rebuild = Request::builder()
                            .method(modified.method.as_str())
                            .uri(modified.url.parse::<Uri>()?);
                        for (k, v) in &modified.headers {
                            rebuild = rebuild.header(k, v);
                        }
                        let mod_body: BoxBody = if let Some(ref b) = modified.body {
                            Full::new(Bytes::copy_from_slice(b))
                                .map_err(|never| match never {})
                                .boxed()
                        } else {
                            Empty::<Bytes>::new()
                                .map_err(|never| match never {})
                                .boxed()
                        };
                        let rebuilt_req = rebuild.body(mod_body)?;
                        let response = get_https_client()
                            .request(rebuilt_req)
                            .await
                            .map_err(|e| anyhow::anyhow!("forward HTTPS request failed: {e}"))?;
                        let latency_ms = start.elapsed().as_millis() as u64;
                        let resp_status = response.status();
                        let resp_status_text = resp_status
                            .canonical_reason()
                            .unwrap_or("Unknown")
                            .to_string();
                        let resp_status_u16 = resp_status.as_u16();
                        let mut resp_headers = std::collections::HashMap::new();
                        for (k, v) in response.headers() {
                            if let Ok(val) = v.to_str() {
                                resp_headers.insert(k.as_str().to_lowercase(), val.to_string());
                            }
                        }
                        let (resp_parts, resp_body) = response.into_parts();
                        let resp_body_bytes = resp_body
                            .collect()
                            .await
                            .map_err(|e| anyhow::anyhow!("failed to read response body: {e}"))?
                            .to_bytes();
                        let captured_resp = CapturedResponse {
                            id: uuid::Uuid::new_v4().to_string(),
                            request_id: request_id.clone(),
                            status: resp_status_u16,
                            status_text: resp_status_text,
                            headers: resp_headers,
                            body: if resp_body_bytes.is_empty() {
                                None
                            } else {
                                Some(resp_body_bytes.to_vec())
                            },
                            timestamp: chrono::Utc::now(),
                            latency_ms,
                        };
                        let exchange = Exchange {
                            request: captured_req,
                            response: Some(captured_resp),
                        };
                        let _ = tx.send(exchange).await;
                        let mut client_resp = Response::builder().status(resp_parts.status);
                        for (k, v) in &resp_parts.headers {
                            client_resp = client_resp.header(k, v);
                        }
                        let body: BoxBody = if resp_body_bytes.is_empty() {
                            Empty::<Bytes>::new()
                                .map_err(|never| match never {})
                                .boxed()
                        } else {
                            Full::new(resp_body_bytes)
                                .map_err(|never| match never {})
                                .boxed()
                        };
                        return Ok(client_resp.body(body)?);
                    }
                }
                crate::intercept::InterceptDecision::Forward => {}
            }
        }
    }

    // Build forwarded request
    let mut outbound_req = Request::builder().method(parts.method).uri(uri.clone());

    for (k, v) in &parts.headers {
        outbound_req = outbound_req.header(k, v);
    }

    let outbound_body: BoxBody = if req_body_bytes.is_empty() {
        Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed()
    } else {
        Full::new(req_body_bytes)
            .map_err(|never| match never {})
            .boxed()
    };

    let outbound_req = outbound_req.body(outbound_body)?;

    // Forward via HTTPS client
    let response = get_https_client()
        .request(outbound_req)
        .await
        .map_err(|e| anyhow::anyhow!("forward HTTPS request failed: {e}"))?;

    let latency_ms = start.elapsed().as_millis() as u64;

    // Handle WebSocket upgrade
    if is_ws_upgrade && response.status() == hyper::StatusCode::SWITCHING_PROTOCOLS {
        eprintln!("[ledger] WebSocket upgrade accepted: {} {}", method, uri);

        // Capture the 101 response
        let mut upgrade_resp_headers = std::collections::HashMap::new();
        for (k, v) in response.headers() {
            if let Ok(val) = v.to_str() {
                upgrade_resp_headers.insert(k.as_str().to_lowercase(), val.to_string());
            }
        }
        let captured_upgrade_resp = CapturedResponse {
            id: uuid::Uuid::new_v4().to_string(),
            request_id: request_id.clone(),
            status: 101,
            status_text: "Switching Protocols".to_string(),
            headers: upgrade_resp_headers,
            body: None,
            timestamp: chrono::Utc::now(),
            latency_ms,
        };

        // Store the upgrade handshake
        let _ = tx
            .send(Exchange {
                request: captured_req,
                response: Some(captured_upgrade_resp),
            })
            .await;

        // Capture response headers before moving response
        let response_headers: Vec<(hyper::header::HeaderName, hyper::header::HeaderValue)> =
            response
                .headers()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();

        // Get upstream upgraded I/O
        let upstream_upgrade = hyper::upgrade::on(response);

        // Build 101 response for client
        let mut client_resp = Response::builder().status(hyper::StatusCode::SWITCHING_PROTOCOLS);
        for (k, v) in &response_headers {
            client_resp = client_resp.header(k, v);
        }

        // Spawn bridge task
        let tx_bridge = tx.clone();
        let session_bridge = session.clone();
        let authority_bridge = authority.clone();
        let request_id_bridge = request_id.clone();
        tokio::spawn(async move {
            let Some(on_upgrade) = on_upgrade else {
                eprintln!("[ledger] WebSocket upgrade not available");
                return;
            };
            match on_upgrade.await {
                Ok(client_upgraded) => {
                    match upstream_upgrade.await {
                        Ok(upstream_upgraded) => {
                            let client_io = hyper_util::rt::TokioIo::new(client_upgraded);
                            let upstream_io = hyper_util::rt::TokioIo::new(upstream_upgraded);

                            let client_ws = match tokio_tungstenite::accept_async(client_io).await {
                                Ok(ws) => ws,
                                Err(e) => {
                                    eprintln!("[ledger] WebSocket accept failed: {e}");
                                    return;
                                }
                            };

                            // For upstream, we need to do the client handshake
                            let upstream_uri =
                                format!("wss://{}{}", authority_bridge, path_and_query);
                            let upstream_req = match upstream_uri.parse::<Uri>() {
                                Ok(u) => u,
                                Err(e) => {
                                    eprintln!("[ledger] WebSocket upstream URI parse failed: {e}");
                                    return;
                                }
                            };
                            let upstream_ws =
                                match tokio_tungstenite::client_async(upstream_req, upstream_io)
                                    .await
                                {
                                    Ok((ws, _)) => ws,
                                    Err(e) => {
                                        eprintln!(
                                            "[ledger] WebSocket upstream connect failed: {e}"
                                        );
                                        return;
                                    }
                                };

                            if let Err(e) = crate::websocket::proxy_websocket_bridge(
                                client_ws,
                                upstream_ws,
                                request_id_bridge,
                                tx_bridge,
                                session_bridge,
                                authority_bridge,
                            )
                            .await
                            {
                                eprintln!("[ledger] WebSocket bridge error: {e}");
                            }
                        }
                        Err(e) => eprintln!("[ledger] Upstream upgrade failed: {e}"),
                    }
                }
                Err(e) => eprintln!("[ledger] Client upgrade failed: {e}"),
            }
        });

        let body: BoxBody = Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed();
        return Ok(client_resp.body(body)?);
    }

    // Capture response
    let resp_status = response.status();
    let resp_status_text = resp_status
        .canonical_reason()
        .unwrap_or("Unknown")
        .to_string();
    let resp_status_u16 = resp_status.as_u16();

    let mut resp_headers = std::collections::HashMap::new();
    for (k, v) in response.headers() {
        if let Ok(val) = v.to_str() {
            resp_headers.insert(k.as_str().to_lowercase(), val.to_string());
        }
    }

    // Collect response body
    let (resp_parts, resp_body) = response.into_parts();
    let resp_body_bytes = resp_body
        .collect()
        .await
        .map_err(|e| anyhow::anyhow!("failed to read response body: {e}"))?
        .to_bytes();

    let captured_resp = CapturedResponse {
        id: uuid::Uuid::new_v4().to_string(),
        request_id: request_id.clone(),
        status: resp_status_u16,
        status_text: resp_status_text,
        headers: resp_headers,
        body: if resp_body_bytes.is_empty() {
            None
        } else {
            Some(resp_body_bytes.to_vec())
        },
        timestamp: chrono::Utc::now(),
        latency_ms,
    };

    let exchange = Exchange {
        request: captured_req,
        response: Some(captured_resp),
    };

    let _ = tx.send(exchange).await;

    // Rebuild response for client
    let mut client_resp = Response::builder().status(resp_parts.status);
    for (k, v) in &resp_parts.headers {
        client_resp = client_resp.header(k, v);
    }

    let body: BoxBody = if resp_body_bytes.is_empty() {
        Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed()
    } else {
        Full::new(resp_body_bytes)
            .map_err(|never| match never {})
            .boxed()
    };

    let client_resp = client_resp.body(body)?;
    Ok(client_resp)
}

async fn handle_request(
    req: Request<Incoming>,
    tx: mpsc::Sender<Exchange>,
    session: String,
    intercept_rules: Option<Vec<crate::intercept::InterceptRule>>,
) -> Result<Response<BoxBody>, Infallible> {
    match proxy_request(req, tx, session, intercept_rules).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            eprintln!("[ledger] proxy error: {e}");
            let body = Full::new(Bytes::from(format!("ledger proxy error: {e}")))
                .map_err(|never| match never {})
                .boxed();
            Ok(Response::builder()
                .status(502)
                .body(body)
                .expect("BoxBody should always construct successfully"))
        }
    }
}

async fn proxy_request(
    req: Request<Incoming>,
    tx: mpsc::Sender<Exchange>,
    session: String,
    intercept_rules: Option<Vec<crate::intercept::InterceptRule>>,
) -> Result<Response<BoxBody>> {
    let start = Instant::now();

    // Build outgoing request
    let (parts, body) = req.into_parts();

    // Capture request headers BEFORE we consume them
    let mut req_headers = std::collections::HashMap::new();
    for (k, v) in &parts.headers {
        if let Ok(val) = v.to_str() {
            req_headers.insert(k.as_str().to_lowercase(), val.to_string());
        }
    }

    let uri = build_target_uri(&parts.uri, &req_headers)?;
    let host = uri.host().unwrap_or("unknown").to_string();
    let path = uri.path().to_string();
    let method = parts.method.to_string();

    // Collect request body
    let req_body_bytes = body
        .collect()
        .await
        .map_err(|e| anyhow::anyhow!("failed to read request body: {e}"))?
        .to_bytes();

    let request_id = uuid::Uuid::new_v4().to_string();
    let captured_req = CapturedRequest {
        id: request_id.clone(),
        method: method.clone(),
        url: uri.to_string(),
        path: path.clone(),
        host: host.clone(),
        headers: req_headers,
        body: if req_body_bytes.is_empty() {
            None
        } else {
            Some(req_body_bytes.to_vec())
        },
        timestamp: chrono::Utc::now(),
        session: session.clone(),
    };

    // Check intercept rules
    if let Some(ref rules) = intercept_rules {
        let engine = crate::intercept::InterceptEngine::new(rules.clone(), true);
        if engine.should_intercept(&captured_req) {
            let (decision, modified_req) = engine.prompt(&captured_req).await?;
            match decision {
                crate::intercept::InterceptDecision::Drop => {
                    let body = Full::new(Bytes::from("Intercepted and dropped by ledger"))
                        .map_err(|never| match never {})
                        .boxed();
                    return Ok(Response::builder().status(502).body(body)?);
                }
                crate::intercept::InterceptDecision::Modify => {
                    if let Some(ref modified) = modified_req {
                        let mut rebuild = Request::builder()
                            .method(modified.method.as_str())
                            .uri(modified.url.parse::<Uri>()?);
                        for (k, v) in &modified.headers {
                            rebuild = rebuild.header(k, v);
                        }
                        let mod_body: BoxBody = if let Some(ref b) = modified.body {
                            Full::new(Bytes::copy_from_slice(b))
                                .map_err(|never| match never {})
                                .boxed()
                        } else {
                            Empty::<Bytes>::new()
                                .map_err(|never| match never {})
                                .boxed()
                        };
                        let rebuilt_req = rebuild.body(mod_body)?;
                        let response = get_client()
                            .request(rebuilt_req)
                            .await
                            .map_err(|e| anyhow::anyhow!("forward request failed: {e}"))?;
                        let latency_ms = start.elapsed().as_millis() as u64;
                        let resp_status = response.status();
                        let resp_status_text = resp_status
                            .canonical_reason()
                            .unwrap_or("Unknown")
                            .to_string();
                        let resp_status_u16 = resp_status.as_u16();
                        let mut resp_headers = std::collections::HashMap::new();
                        for (k, v) in response.headers() {
                            if let Ok(val) = v.to_str() {
                                resp_headers.insert(k.as_str().to_lowercase(), val.to_string());
                            }
                        }
                        let (resp_parts, resp_body) = response.into_parts();
                        let resp_body_bytes = resp_body
                            .collect()
                            .await
                            .map_err(|e| anyhow::anyhow!("failed to read response body: {e}"))?
                            .to_bytes();
                        let captured_resp = CapturedResponse {
                            id: uuid::Uuid::new_v4().to_string(),
                            request_id: request_id.clone(),
                            status: resp_status_u16,
                            status_text: resp_status_text,
                            headers: resp_headers,
                            body: if resp_body_bytes.is_empty() {
                                None
                            } else {
                                Some(resp_body_bytes.to_vec())
                            },
                            timestamp: chrono::Utc::now(),
                            latency_ms,
                        };
                        let exchange = Exchange {
                            request: captured_req,
                            response: Some(captured_resp),
                        };
                        let _ = tx.send(exchange).await;
                        let mut client_resp = Response::builder().status(resp_parts.status);
                        for (k, v) in &resp_parts.headers {
                            client_resp = client_resp.header(k, v);
                        }
                        let body: BoxBody = if resp_body_bytes.is_empty() {
                            Empty::<Bytes>::new()
                                .map_err(|never| match never {})
                                .boxed()
                        } else {
                            Full::new(resp_body_bytes)
                                .map_err(|never| match never {})
                                .boxed()
                        };
                        return Ok(client_resp.body(body)?);
                    }
                }
                crate::intercept::InterceptDecision::Forward => {}
            }
        }
    }

    // Build forwarded request
    let mut outbound_req = Request::builder().method(parts.method).uri(uri.clone());

    for (k, v) in &parts.headers {
        outbound_req = outbound_req.header(k, v);
    }

    let outbound_body: BoxBody = if req_body_bytes.is_empty() {
        Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed()
    } else {
        Full::new(req_body_bytes)
            .map_err(|never| match never {})
            .boxed()
    };

    let outbound_req = outbound_req.body(outbound_body)?;

    // Forward via hyper client with HTTPS support
    let response = get_client()
        .request(outbound_req)
        .await
        .map_err(|e| anyhow::anyhow!("forward request failed: {e}"))?;

    let latency_ms = start.elapsed().as_millis() as u64;

    // Capture response
    let resp_status = response.status();
    let resp_status_text = resp_status
        .canonical_reason()
        .unwrap_or("Unknown")
        .to_string();
    let resp_status_u16 = resp_status.as_u16();

    let mut resp_headers = std::collections::HashMap::new();
    for (k, v) in response.headers() {
        if let Ok(val) = v.to_str() {
            resp_headers.insert(k.as_str().to_lowercase(), val.to_string());
        }
    }

    // Collect response body
    let (resp_parts, resp_body) = response.into_parts();
    let resp_body_bytes = resp_body
        .collect()
        .await
        .map_err(|e| anyhow::anyhow!("failed to read response body: {e}"))?
        .to_bytes();

    let captured_resp = CapturedResponse {
        id: uuid::Uuid::new_v4().to_string(),
        request_id: request_id.clone(),
        status: resp_status_u16,
        status_text: resp_status_text,
        headers: resp_headers,
        body: if resp_body_bytes.is_empty() {
            None
        } else {
            Some(resp_body_bytes.to_vec())
        },
        timestamp: chrono::Utc::now(),
        latency_ms,
    };

    let exchange = Exchange {
        request: captured_req,
        response: Some(captured_resp),
    };

    let _ = tx.send(exchange).await;

    // Rebuild response for client
    let mut client_resp = Response::builder().status(resp_parts.status);
    for (k, v) in &resp_parts.headers {
        client_resp = client_resp.header(k, v);
    }

    let body: BoxBody = if resp_body_bytes.is_empty() {
        Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed()
    } else {
        Full::new(resp_body_bytes)
            .map_err(|never| match never {})
            .boxed()
    };

    let client_resp = client_resp.body(body)?;
    Ok(client_resp)
}

/// Get or create the shared HTTP client with TLS support.
fn get_client() -> &'static Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    BoxBody,
> {
    static CLIENT: std::sync::OnceLock<
        Client<
            hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
            BoxBody,
        >,
    > = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .expect("no native root CA certificates found")
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();
        Client::builder(TokioExecutor::new()).build::<_, BoxBody>(https)
    })
}

/// Get or create the shared HTTPS-only client for upstream connections in MITM mode.
fn get_https_client() -> &'static Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    BoxBody,
> {
    static CLIENT: std::sync::OnceLock<
        Client<
            hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
            BoxBody,
        >,
    > = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .expect("no native root CA certificates found")
            .https_only()
            .enable_http1()
            .enable_http2()
            .build();
        Client::builder(TokioExecutor::new()).build::<_, BoxBody>(https)
    })
}

fn build_target_uri(uri: &Uri, headers: &std::collections::HashMap<String, String>) -> Result<Uri> {
    // If the URI has a scheme, it's already absolute
    if uri.scheme().is_some() {
        return Ok(uri.clone());
    }

    // Otherwise, it's a relative URI from a proxy request — we need the Host header
    let host = headers
        .get("host")
        .cloned()
        .or_else(|| uri.host().map(|h| h.to_string()))
        .ok_or_else(|| anyhow::anyhow!("missing Host header in proxy request"))?;

    let path_and_query = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");

    let scheme = if uri.port_u16() == Some(443) || host.ends_with(":443") {
        "https"
    } else {
        "http"
    };

    // Strip default port from host to avoid invalid authority like "host:443"
    let host_clean = if scheme == "https" && host.ends_with(":443") {
        host.strip_suffix(":443").unwrap_or(&host).to_string()
    } else if scheme == "http" && host.ends_with(":80") {
        host.strip_suffix(":80").unwrap_or(&host).to_string()
    } else {
        host.to_string()
    };

    let authority = host_clean;

    let target = format!("{}://{}{}", scheme, authority, path_and_query);
    target
        .parse::<Uri>()
        .map_err(|e| anyhow::anyhow!("invalid target URI: {e}"))
}

pub fn parse_addr(addr: &str) -> Result<SocketAddr> {
    addr.parse::<SocketAddr>()
        .map_err(|_| anyhow::anyhow!("invalid address: {addr}"))
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_addr_valid() {
        let addr = parse_addr("127.0.0.1:8080").unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert_eq!(addr.port(), 8080);
    }

    #[test]
    fn test_parse_addr_invalid() {
        assert!(parse_addr("not-an-address").is_err());
    }

    #[test]
    fn test_build_target_uri_absolute() {
        let uri = "http://example.com/path".parse::<Uri>().unwrap();
        let headers = std::collections::HashMap::new();
        let result = build_target_uri(&uri, &headers).unwrap();
        assert_eq!(result.to_string(), "http://example.com/path");
    }

    #[test]
    fn test_build_target_uri_from_host_header() {
        let uri = "/path".parse::<Uri>().unwrap();
        let mut headers = std::collections::HashMap::new();
        headers.insert("host".to_string(), "api.example.com".to_string());
        let result = build_target_uri(&uri, &headers).unwrap();
        assert_eq!(result.to_string(), "http://api.example.com/path");
    }

    #[test]
    fn test_build_target_uri_https_port() {
        let uri = "/path".parse::<Uri>().unwrap();
        let mut headers = std::collections::HashMap::new();
        headers.insert("host".to_string(), "secure.example.com:443".to_string());
        let result = build_target_uri(&uri, &headers).unwrap();
        assert_eq!(result.to_string(), "https://secure.example.com/path");
    }
}
