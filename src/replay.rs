//! Request replay engine.

use std::collections::HashMap;
use std::convert::Infallible;
use std::time::Duration;

use anyhow::{Context, Result};
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Bytes;
use hyper::{Request, Uri};
use hyper_util::client::legacy::Client;
use sqlx::SqlitePool;

use crate::db;
use crate::models::{CapturedRequest, CapturedResponse, Exchange};

pub struct ReplayEngine {
    pool: SqlitePool,
}

impl ReplayEngine {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn replay_by_id(
        &self,
        request_id: &str,
        count: u32,
        dry_run: bool,
        diff: bool,
        pre_script: Option<&str>,
        post_script: Option<&str>,
    ) -> Result<()> {
        let exchange = db::get_request_by_id(&self.pool, request_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("request not found: {request_id}"))?;

        for i in 0..count {
            if count > 1 {
                eprintln!("[ledger] replay {}/{}: {}", i + 1, count, request_id);
            }
            self.replay_exchange(&exchange, dry_run, diff, pre_script, post_script).await?;

            if i + 1 < count {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        Ok(())
    }

    pub async fn replay_by_id_with_edit(
        &self,
        request_id: &str,
        count: u32,
        dry_run: bool,
        diff: bool,
        pre_script: Option<&str>,
        post_script: Option<&str>,
    ) -> Result<()> {
        let exchange = db::get_request_by_id(&self.pool, request_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("request not found: {request_id}"))?;

        let edited_req = edit_request_in_editor(&exchange.request).await?;

        // Build a new exchange with the edited request but original response for diff
        let edited_exchange = Exchange {
            request: edited_req,
            response: exchange.response.clone(),
        };

        for i in 0..count {
            if count > 1 {
                eprintln!("[ledger] replay {}/{}: {} (edited)", i + 1, count, request_id);
            }
            self.replay_exchange(&edited_exchange, dry_run, diff, pre_script, post_script).await?;

            if i + 1 < count {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }

        Ok(())
    }

    pub async fn replay_filtered(
        &self,
        filter_expr: &str,
        dry_run: bool,
        diff: bool,
        pre_script: Option<&str>,
        post_script: Option<&str>,
    ) -> Result<()> {
        let filter = parse_filter_expr(filter_expr)?;
        let exchanges = db::search_exchanges(&self.pool, &filter).await?;

        if exchanges.is_empty() {
            eprintln!("[ledger] no requests matched filter: {filter_expr}");
            return Ok(());
        }

        eprintln!(
            "[ledger] replaying {} requests (dry_run={dry_run}, diff={diff})",
            exchanges.len()
        );

        for exchange in &exchanges {
            self.replay_exchange(exchange, dry_run, diff, pre_script, post_script).await?;
        }

        Ok(())
    }

    async fn replay_exchange(
        &self,
        exchange: &Exchange,
        dry_run: bool,
        diff: bool,
        pre_script: Option<&str>,
        post_script: Option<&str>,
    ) -> Result<()> {
        let mut req = exchange.request.clone();

        // Run pre-request script if provided
        if let Some(script) = pre_script {
            let engine = crate::scripts::ScriptEngine::new()?;
            let mods = engine.run_pre_request(script, &req)?;
            mods.apply(&mut req);
        }

        let http_req = self.build_http_request(&req).await?;

        if dry_run {
            eprintln!("[dry-run] {} {}", req.method, req.url);
            for (k, v) in &req.headers {
                eprintln!("[dry-run] {k}: {v}");
            }
            if let Some(ref body) = req.body {
                if let Ok(text) = std::str::from_utf8(body) {
                    eprintln!("[dry-run] body: {text}");
                } else {
                    eprintln!("[dry-run] body: <{} bytes>", body.len());
                }
            }
            return Ok(());
        }

        let client = get_https_client();
        let start = std::time::Instant::now();

        let response = client
            .request(http_req)
            .await
            .map_err(|e| anyhow::anyhow!("replay request failed: {e}"))?;

        let latency_ms = start.elapsed().as_millis() as u64;
        let status = response.status();

        // Collect response body for display/diff
        let (resp_parts, body) = response.into_parts();
        let body_bytes = body
            .collect()
            .await
            .map_err(|e| anyhow::anyhow!("failed to read replay response: {e}"))?
            .to_bytes();

        eprintln!(
            "[ledger] replayed {} {} -> {} ({} ms, {} bytes)",
            req.method,
            req.url,
            status.as_u16(),
            latency_ms,
            body_bytes.len()
        );

        // Build captured response for diff and post-script
        let replayed_resp = CapturedResponse {
            id: uuid::Uuid::new_v4().to_string(),
            request_id: req.id.clone(),
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or("Unknown").to_string(),
            headers: resp_parts
                .headers
                .iter()
                .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.as_str().to_lowercase(), s.to_string())))
                .collect(),
            body: if body_bytes.is_empty() { None } else { Some(body_bytes.to_vec()) },
            timestamp: chrono::Utc::now(),
            latency_ms,
        };

        if diff {
            if let Some(ref original_resp) = exchange.response {
                print_diff(original_resp, &replayed_resp);
            } else {
                eprintln!("[ledger] diff: no original response to compare against");
            }
        }

        // Run post-response script if provided
        if let Some(script) = post_script {
            let engine = crate::scripts::ScriptEngine::new()?;
            if let Err(e) = engine.run_post_response(script, &replayed_resp) {
                eprintln!("[ledger] post-response script error: {e}");
            }
        }

        Ok(())
    }

    /// Replay a request and return the captured response (for chaining).
    pub async fn replay_exchange_for_chain(
        &self,
        exchange: &Exchange,
    ) -> Result<CapturedResponse> {
        let req = &exchange.request;
        let http_req = self.build_http_request(req).await?;

        let client = get_https_client();
        let start = std::time::Instant::now();

        let response = client
            .request(http_req)
            .await
            .map_err(|e| anyhow::anyhow!("replay request failed: {e}"))?;

        let latency_ms = start.elapsed().as_millis() as u64;
        let status = response.status();

        let (resp_parts, body) = response.into_parts();
        let body_bytes = body
            .collect()
            .await
            .map_err(|e| anyhow::anyhow!("failed to read replay response: {e}"))?
            .to_bytes();

        eprintln!(
            "[ledger] replayed {} {} -> {} ({} ms, {} bytes)",
            req.method,
            req.url,
            status.as_u16(),
            latency_ms,
            body_bytes.len()
        );

        Ok(CapturedResponse {
            id: uuid::Uuid::new_v4().to_string(),
            request_id: req.id.clone(),
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or("Unknown").to_string(),
            headers: resp_parts
                .headers
                .iter()
                .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.as_str().to_lowercase(), s.to_string())))
                .collect(),
            body: if body_bytes.is_empty() { None } else { Some(body_bytes.to_vec()) },
            timestamp: chrono::Utc::now(),
            latency_ms,
        })
    }

    async fn build_http_request(
        &self,
        captured: &CapturedRequest,
    ) -> Result<Request<BoxBody>> {
        let uri: Uri = captured
            .url
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid URL: {e}"))?;

        let mut req = Request::builder().method(captured.method.as_str()).uri(uri);

        for (k, v) in &captured.headers {
            // Skip hop-by-hop and proxy-related headers
            let lower = k.to_lowercase();
            if lower == "proxy-connection"
                || lower == "proxy-authorization"
                || lower == "connection"
                || lower == "keep-alive"
                || lower == "transfer-encoding"
                || lower == "upgrade"
            {
                continue;
            }
            req = req.header(k, v);
        }

        let body: BoxBody = if let Some(ref bytes) = captured.body {
            Full::new(Bytes::copy_from_slice(bytes))
                .map_err(|never| match never {})
                .boxed()
        } else {
            Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed()
        };

        Ok(req.body(body)?)
    }
}

type BoxBody = http_body_util::combinators::BoxBody<Bytes, Infallible>;

/// Get or create the shared HTTPS client for replay.
fn get_https_client() -> &'static Client<hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>, BoxBody> {
    static CLIENT: std::sync::OnceLock<Client<hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>, BoxBody>> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .expect("no native root CA certificates found")
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();
        Client::builder(hyper_util::rt::TokioExecutor::new()).build::<_, BoxBody>(https)
    })
}

// ── Diff ────────────────────────────────────────────────────────────────────

fn print_diff(original: &CapturedResponse, replayed: &CapturedResponse) {
    eprintln!("\n--- diff ---");

    // Status diff
    if original.status != replayed.status {
        eprintln!(
            "  status: {} -> {} {}",
            original.status, replayed.status,
            if replayed.status == original.status { "(same)" } else { "(CHANGED)" }
        );
    } else {
        eprintln!("  status: {} (same)", original.status);
    }

    // Header diffs
    let header_changes = diff_headers(&original.headers, &replayed.headers);
    if !header_changes.is_empty() {
        eprintln!("  headers:");
        for change in &header_changes {
            match change {
                HeaderChange::Added(k, v) => eprintln!("    + {k}: {v}"),
                HeaderChange::Removed(k, v) => eprintln!("    - {k}: {v}"),
                HeaderChange::Changed(k, old, new) => eprintln!("    ~ {k}: {old} -> {new}"),
            }
        }
    } else {
        eprintln!("  headers: (same)");
    }

    // Body diff
    match (&original.body, &replayed.body) {
        (None, None) => eprintln!("  body: (same, both empty)"),
        (Some(_), None) => eprintln!("  body: original had body, replay returned empty (CHANGED)"),
        (None, Some(_)) => eprintln!("  body: original was empty, replay returned body (CHANGED)"),
        (Some(orig), Some(repl)) => {
            let is_json = original.headers.get("content-type")
                .or_else(|| original.headers.get("content-type"))
                .map(|ct| ct.contains("application/json"))
                .unwrap_or(false);

            if is_json {
                match (serde_json::from_slice::<serde_json::Value>(orig), serde_json::from_slice::<serde_json::Value>(repl)) {
                    (Ok(orig_val), Ok(repl_val)) => {
                        if orig_val == repl_val {
                            eprintln!("  body: JSON (same)");
                        } else {
                            eprintln!("  body: JSON (CHANGED)");
                            print_json_diff(&orig_val, &repl_val, "  ");
                        }
                    }
                    _ => print_raw_body_diff(orig, repl),
                }
            } else {
                print_raw_body_diff(orig, repl);
            }
        }
    }

    eprintln!("--- end diff ---\n");
}

#[derive(Debug)]
enum HeaderChange {
    Added(String, String),
    Removed(String, String),
    Changed(String, String, String),
}

fn diff_headers(
    original: &HashMap<String, String>,
    replayed: &HashMap<String, String>,
) -> Vec<HeaderChange> {
    let mut changes = Vec::new();

    // Check for changed or removed headers
    for (k, v_orig) in original {
        match replayed.get(k) {
            Some(v_repl) if v_repl != v_orig => {
                changes.push(HeaderChange::Changed(k.clone(), v_orig.clone(), v_repl.clone()));
            }
            None => {
                changes.push(HeaderChange::Removed(k.clone(), v_orig.clone()));
            }
            _ => {}
        }
    }

    // Check for added headers
    for (k, v_repl) in replayed {
        if !original.contains_key(k) {
            changes.push(HeaderChange::Added(k.clone(), v_repl.clone()));
        }
    }

    changes
}

fn print_raw_body_diff(original: &[u8], replayed: &[u8]) {
    if original == replayed {
        eprintln!("  body: (same, {} bytes)", original.len());
    } else {
        eprintln!(
            "  body: (CHANGED) original={} bytes, replayed={} bytes",
            original.len(),
            replayed.len()
        );
        if let (Ok(orig_str), Ok(repl_str)) = (std::str::from_utf8(original), std::str::from_utf8(replayed)) {
            if orig_str.lines().count() <= 20 && repl_str.lines().count() <= 20 {
                eprintln!("  --- original ---");
                for line in orig_str.lines() {
                    eprintln!("    {line}");
                }
                eprintln!("  --- replayed ---");
                for line in repl_str.lines() {
                    eprintln!("    {line}");
                }
            } else {
                eprintln!("  (bodies too large for inline diff)");
            }
        } else {
            eprintln!("  (binary bodies, no text diff)");
        }
    }
}

fn print_json_diff(original: &serde_json::Value, replayed: &serde_json::Value, indent: &str) {
    match (original, replayed) {
        (serde_json::Value::Object(orig_map), serde_json::Value::Object(repl_map)) => {
            let all_keys: std::collections::HashSet<_> = orig_map.keys().chain(repl_map.keys()).collect();
            for key in all_keys {
                match (orig_map.get(key), repl_map.get(key)) {
                    (Some(v1), Some(v2)) if v1 != v2 => {
                        eprintln!("{indent}  .{key}:");
                        print_json_diff(v1, v2, &format!("{indent}    "));
                    }
                    (Some(v), None) => {
                        eprintln!("{indent}  - {key}: {v}");
                    }
                    (None, Some(v)) => {
                        eprintln!("{indent}  + {key}: {v}");
                    }
                    _ => {}
                }
            }
        }
        (v1, v2) => {
            eprintln!("{indent}  {v1} -> {v2}");
        }
    }
}

fn parse_filter_expr(expr: &str) -> Result<crate::models::Filter> {
    let mut filter = crate::models::Filter::default();

    for part in expr.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let Some((key, value)) = part.split_once('=') else {
            anyhow::bail!("invalid filter expression: '{part}' (expected key=value)");
        };

        match key.trim().to_lowercase().as_str() {
            "method" => filter.method = Some(value.trim().to_uppercase()),
            "path" => filter.path_pattern = Some(value.trim().to_string()),
            "host" => filter.host = Some(value.trim().to_string()),
            "status" => {
                filter.status_code = Some(
                    value
                        .trim()
                        .parse()
                        .map_err(|e| anyhow::anyhow!("invalid status code: {e}"))?,
                )
            }
            "body" => filter.body_contains = Some(value.trim().to_string()),
            _ => anyhow::bail!("unknown filter key: '{key}'"),
        }
    }

    Ok(filter)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_filter_expr() {
        let filter = parse_filter_expr("method=GET,host=api.example.com").unwrap();
        assert_eq!(filter.method, Some("GET".to_string()));
        assert_eq!(filter.host, Some("api.example.com".to_string()));
    }

    #[test]
    fn test_parse_filter_expr_status() {
        let filter = parse_filter_expr("status=404").unwrap();
        assert_eq!(filter.status_code, Some(404));
    }

    #[test]
    fn test_parse_filter_expr_invalid() {
        assert!(parse_filter_expr("invalid").is_err());
    }

    #[test]
    fn test_diff_headers_same() {
        let mut h = HashMap::new();
        h.insert("content-type".to_string(), "application/json".to_string());
        let changes = diff_headers(&h, &h);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_diff_headers_changed() {
        let mut orig = HashMap::new();
        orig.insert("content-type".to_string(), "application/json".to_string());
        let mut repl = HashMap::new();
        repl.insert("content-type".to_string(), "text/html".to_string());
        let changes = diff_headers(&orig, &repl);
        assert_eq!(changes.len(), 1);
        assert!(matches!(&changes[0], HeaderChange::Changed(k, _, _) if k == "content-type"));
    }

    #[test]
    fn test_diff_headers_added_removed() {
        let mut orig = HashMap::new();
        orig.insert("x-old".to_string(), "1".to_string());
        let mut repl = HashMap::new();
        repl.insert("x-new".to_string(), "2".to_string());
        let changes = diff_headers(&orig, &repl);
        assert_eq!(changes.len(), 2);
    }

    #[tokio::test]
    async fn test_build_http_request_with_body() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let pool = crate::db::init_db(&db_path).await.unwrap();

        let engine = ReplayEngine::new(pool);

        let req = CapturedRequest {
            id: "test".to_string(),
            method: "POST".to_string(),
            url: "https://httpbin.org/post".to_string(),
            path: "/post".to_string(),
            host: "httpbin.org".to_string(),
            headers: {
                let mut h = HashMap::new();
                h.insert("content-type".to_string(), "application/json".to_string());
                h
            },
            body: Some(br#"{"test":true}"#.to_vec()),
            timestamp: chrono::Utc::now(),
            session: "default".to_string(),
        };

        let http_req = engine.build_http_request(&req).await.unwrap();

        assert_eq!(http_req.method().as_str(), "POST");
        assert_eq!(http_req.uri().to_string(), "https://httpbin.org/post");

        // Verify body is present by collecting it
        let body_bytes = http_body_util::BodyExt::collect(http_req.into_body())
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(body_bytes.as_ref(), br#"{"test":true}"#);
    }

    #[test]
    fn test_edit_request_roundtrip() {
        let req = CapturedRequest {
            id: "test-id".to_string(),
            method: "POST".to_string(),
            url: "https://api.example.com/users".to_string(),
            path: "/users".to_string(),
            host: "api.example.com".to_string(),
            headers: {
                let mut h = HashMap::new();
                h.insert("content-type".to_string(), "application/json".to_string());
                h.insert("authorization".to_string(), "Bearer token123".to_string());
                h
            },
            body: Some(br#"{"name":"Alice"}"#.to_vec()),
            timestamp: chrono::Utc::now(),
            session: "default".to_string(),
        };

        // Serialize and deserialize to verify roundtrip
        let json = serde_json::to_string_pretty(&req).unwrap();
        let deserialized: CapturedRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.method, req.method);
        assert_eq!(deserialized.url, req.url);
        assert_eq!(deserialized.headers, req.headers);
        assert_eq!(deserialized.body, req.body);
    }

    #[test]
    fn test_edit_request_modify_body() {
        let req = CapturedRequest {
            id: "test-id".to_string(),
            method: "POST".to_string(),
            url: "https://api.example.com/users".to_string(),
            path: "/users".to_string(),
            host: "api.example.com".to_string(),
            headers: {
                let mut h = HashMap::new();
                h.insert("content-type".to_string(), "application/json".to_string());
                h
            },
            body: Some(br#"{"name":"Alice"}"#.to_vec()),
            timestamp: chrono::Utc::now(),
            session: "default".to_string(),
        };

        let json = serde_json::to_string_pretty(&req).unwrap();
        // The body is serialized as base64 in JSON, so we need to modify the base64
        // Actually serde serializes Vec<u8> as an array of numbers by default
        // Let's just verify the roundtrip works and we can modify the JSON
        let deserialized: CapturedRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.body, req.body);

        // Modify the method via JSON string replacement
        let modified = json.replace("\"POST\"", "\"PUT\"");
        let deserialized2: CapturedRequest = serde_json::from_str(&modified).unwrap();
        assert_eq!(deserialized2.method, "PUT");
    }
}

// ── Request Editor ──────────────────────────────────────────────────────────

/// Serialize a request to JSON, open it in $EDITOR, parse it back.
async fn edit_request_in_editor(req: &CapturedRequest) -> Result<CapturedRequest> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());

    let json = serde_json::to_string_pretty(req)
        .context("serializing request to JSON")?;

    let mut tmp = tempfile::NamedTempFile::with_suffix(".json")?;
    std::io::Write::write_all(&mut tmp, json.as_bytes())?;
    let path = tmp.path().to_path_buf();

    // Need to close the file so the editor can open it
    drop(tmp);

    let status = tokio::process::Command::new(&editor)
        .arg(&path)
        .status()
        .await
        .with_context(|| format!("failed to spawn editor: {editor}"))?;

    if !status.success() {
        anyhow::bail!("editor exited with non-zero status");
    }

    let edited_json = tokio::fs::read_to_string(&path)
        .await
        .context("reading edited request file")?;

    let edited_req: CapturedRequest = serde_json::from_str(&edited_json)
        .context("parsing edited request JSON")?;

    Ok(edited_req)
}
