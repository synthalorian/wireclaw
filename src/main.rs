mod cert;
mod chain;
mod cli;
mod config;
mod dashboard;
mod db;
mod diff;
mod export;
mod intercept;
mod logger;
mod models;
mod openapi;
mod perf;
mod proxy;
mod replay;
mod scripts;
mod search;
mod stats;
mod tui;
mod websocket;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
struct CaptureArgs {
    addr: String,
    session: String,
    verbose: bool,
    intercept: bool,
    intercept_rule: Option<String>,
    dashboard: bool,
    dashboard_addr: String,
}

#[derive(Debug, Clone, Default)]
struct ReplayArgs {
    id: Option<String>,
    count: u32,
    dry_run: bool,
    diff: bool,
    edit: bool,
    filter: Option<String>,
    chain: Option<String>,
    pre_script: Option<String>,
    post_script: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install rustls crypto provider (required for rustls 0.23+)
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let cli = cli::Cli::parse();
    let config = config::load_config(&cli.config)?;

    match cli.command {
        cli::Commands::Capture {
            session,
            verbose,
            intercept,
            intercept_rule,
            dashboard,
            dashboard_addr,
        } => {
            run_capture(
                CaptureArgs {
                    addr: cli.addr,
                    session,
                    verbose,
                    intercept,
                    intercept_rule,
                    dashboard,
                    dashboard_addr,
                },
                &config,
            )
            .await
        }
        cli::Commands::Replay {
            id,
            count,
            dry_run,
            diff,
            edit,
            filter,
            chain,
            pre_script,
            post_script,
        } => {
            run_replay(
                ReplayArgs {
                    id,
                    count,
                    dry_run,
                    diff,
                    edit,
                    filter,
                    chain,
                    pre_script,
                    post_script,
                },
                &config,
            )
            .await
        }
        cli::Commands::List {
            session,
            limit,
            headers,
            bodies,
        } => run_list(&session, limit, headers, bodies, &config).await,
        cli::Commands::Search {
            query,
            session,
            field,
        } => run_search(&query, &session, &field, &config).await,
        cli::Commands::Export {
            format,
            session,
            output,
        } => {
            run_export(
                format,
                &session,
                output.as_ref().map(std::path::Path::new),
                &config,
            )
            .await
        }
        cli::Commands::Tui { session } => run_tui(&session, &config).await,
        cli::Commands::Stats { session } => run_stats(&session, &config).await,
        cli::Commands::Init => run_init(&config).await,
        cli::Commands::Ca { command } => run_ca(command, &config).await,
        cli::Commands::WsReplay {
            id,
            session,
            delay_ms,
        } => run_ws_replay(&id, &session, delay_ms, &config).await,
        cli::Commands::Dashboard { session, addr } => run_dashboard(&session, &addr, &config).await,
        cli::Commands::Diff { a, b, session } => run_diff(&a, &b, &session, &config).await,
        cli::Commands::Openapi { session, output } => {
            let output_path = output.as_deref().map(std::path::Path::new);
            run_openapi(&session, output_path, &config).await
        }
    }
}

async fn run_capture(args: CaptureArgs, config: &config::Config) -> Result<()> {
    eprintln!(
        "[wireclaw] starting capture on {}, session={}, verbose={}, intercept={}",
        args.addr, args.session, args.verbose, args.intercept
    );
    let data_dir = config.data_dir.join("sessions");
    let db_path = data_dir.join(format!("{}.db", args.session));
    let pool = db::init_db(&db_path).await?;

    let session_model = models::Session::new(args.session.clone(), db_path.display().to_string());
    db::create_session(&pool, &session_model).await?;

    let listen_addr = proxy::parse_addr(&args.addr)?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<models::Exchange>(256);

    // Optionally launch the dashboard in-process for real-time WebSocket updates.
    let mut dashboard_sender: Option<tokio::sync::broadcast::Sender<dashboard::DashboardEvent>> =
        None;
    let dashboard_handle: Option<tokio::task::JoinHandle<Result<()>>> = if args.dashboard {
        let dashboard_server = dashboard::DashboardServer::new(pool.clone(), args.session.clone());
        dashboard_sender = Some(dashboard_server.sender());
        eprintln!(
            "[wireclaw] launching dashboard at http://{}",
            args.dashboard_addr
        );
        let addr = args.dashboard_addr.clone();
        Some(tokio::spawn(
            async move { dashboard_server.run(&addr).await },
        ))
    } else {
        None
    };

    let logger = if let Some(sender) = dashboard_sender {
        logger::Logger::new(pool.clone()).with_broadcast(sender)
    } else {
        logger::Logger::new(pool.clone())
    };

    let cert_dir = config.data_dir.join("certs");
    let cert_mgr = Arc::new(cert::CertManager::load_or_create(&cert_dir)?);

    // Build intercept rules if enabled
    let intercept_rules = if args.intercept {
        let mut rules = Vec::new();
        if let Some(ref expr) = args.intercept_rule {
            match crate::intercept::InterceptRule::parse(expr) {
                Ok(rule) => rules.push(rule),
                Err(e) => eprintln!("[wireclaw] warning: invalid intercept rule: {e}"),
            }
        }
        // If no specific rule given, intercept everything
        if rules.is_empty() {
            rules.push(crate::intercept::InterceptRule::parse("")?);
        }
        Some(rules)
    } else {
        None
    };

    let proxy = std::sync::Arc::new(proxy::ProxyServer::new(
        listen_addr,
        tx,
        args.session.clone(),
        cert_mgr,
        intercept_rules,
    ));
    let proxy_handle = tokio::spawn(async move { proxy.run().await });

    let logger_handle = tokio::spawn(async move {
        while let Some(exchange) = rx.recv().await {
            // Check if this is a WS frame exchange (method == "WS")
            if exchange.request.method == "WS" {
                // Extract frame info from headers and store it
                let direction = exchange
                    .request
                    .headers
                    .get("x-wireclaw-ws-direction")
                    .map(|s| s.as_str())
                    .unwrap_or("client->server");
                let opcode = exchange
                    .request
                    .headers
                    .get("x-wireclaw-ws-opcode")
                    .cloned()
                    .unwrap_or_else(|| "binary".to_string());
                let ws_direction = if direction == "server->client" {
                    crate::websocket::WsDirection::ServerToClient
                } else {
                    crate::websocket::WsDirection::ClientToServer
                };
                let frame = crate::websocket::WsFrame {
                    id: exchange.request.id.clone(),
                    request_id: exchange.request.id.clone(), // Will be overwritten
                    direction: ws_direction,
                    opcode,
                    payload: exchange.request.body.clone(),
                    timestamp: exchange.request.timestamp,
                };
                if let Err(e) = logger.log_ws_frame(&frame).await {
                    eprintln!("[wireclaw] failed to log ws frame: {e}");
                }
                continue;
            }
            if let Err(e) = logger.log_exchange(&exchange).await {
                eprintln!("[wireclaw] failed to log exchange: {e}");
            }
            if args.verbose {
                eprintln!(
                    "[wireclaw] {} {} {} ({})",
                    exchange.request.method,
                    exchange.request.path,
                    exchange.status_label(),
                    exchange.request.host,
                );
            }
        }
    });

    if let Some(handle) = dashboard_handle {
        tokio::select! {
            r = proxy_handle => r??,
            r = logger_handle => r?,
            r = handle => r??,
        }
    } else {
        tokio::select! {
            r = proxy_handle => r??,
            r = logger_handle => r?,
        }
    }
    Ok(())
}

async fn run_replay(args: ReplayArgs, config: &config::Config) -> Result<()> {
    let db_path = config.data_dir.join("sessions").join("default.db");
    let pool = db::init_db(&db_path).await?;

    if let Some(chain_expr) = args.chain {
        let engine = chain::ChainEngine::new(pool);
        let steps = chain::ChainEngine::parse_chain(&chain_expr)?;
        let vars = engine.replay_chain(&steps, args.dry_run).await?;
        eprintln!("[wireclaw] chain complete. extracted variables:");
        for (k, v) in &vars {
            eprintln!("  ${{{k}}} = {v}");
        }
        return Ok(());
    }

    let engine = replay::ReplayEngine::new(pool);

    match (args.id, args.filter) {
        (Some(request_id), _) => {
            eprintln!(
                "[wireclaw] replaying request {request_id} x{} (dry_run={}, diff={}, edit={})",
                args.count, args.dry_run, args.diff, args.edit
            );
            let pre = args.pre_script.as_deref();
            let post = args.post_script.as_deref();
            if args.edit {
                engine
                    .replay_by_id_with_edit(
                        &request_id,
                        args.count,
                        args.dry_run,
                        args.diff,
                        pre,
                        post,
                    )
                    .await?;
            } else {
                engine
                    .replay_by_id(&request_id, args.count, args.dry_run, args.diff, pre, post)
                    .await?;
            }
        }
        (None, Some(filter_expr)) => {
            eprintln!(
                "[wireclaw] replaying filtered requests: {filter_expr} (dry_run={}, diff={})",
                args.dry_run, args.diff
            );
            engine
                .replay_filtered(
                    &filter_expr,
                    args.dry_run,
                    args.diff,
                    args.pre_script.as_deref(),
                    args.post_script.as_deref(),
                )
                .await?;
        }
        (None, None) => {
            anyhow::bail!("specify --id or --filter for replay");
        }
    }
    Ok(())
}

async fn run_list(
    session: &str,
    limit: usize,
    headers: bool,
    bodies: bool,
    config: &config::Config,
) -> Result<()> {
    let db_path = config
        .data_dir
        .join("sessions")
        .join(format!("{session}.db"));
    let pool = db::init_db(&db_path).await?;
    let exchanges = db::list_exchanges(&pool, session, limit).await?;

    eprintln!(
        "[wireclaw] session={session}, showing {limit} exchanges (headers={headers}, bodies={bodies})"
    );
    for exchange in &exchanges {
        let status = exchange.status_label();
        if headers || bodies {
            eprintln!(
                "  === {} {} {} ({}) ===",
                exchange.request.method, exchange.request.path, status, exchange.request.host
            );
            if headers {
                for (k, v) in &exchange.request.headers {
                    eprintln!("    {k}: {v}");
                }
            }
            if bodies && let Some(ref body) = exchange.request.body {
                eprintln!("    body: {}", String::from_utf8_lossy(body));
            }
            if let Some(ref resp) = exchange.response {
                if headers {
                    for (k, v) in &resp.headers {
                        eprintln!("    resp {k}: {v}");
                    }
                }
                if bodies && let Some(ref body) = resp.body {
                    eprintln!("    resp body: {}", String::from_utf8_lossy(body));
                }
            }
        } else {
            eprintln!(
                "  {} {} {} ({})",
                exchange.request.method, exchange.request.path, status, exchange.request.host,
            );
        }
    }
    eprintln!("[wireclaw] {} exchanges", exchanges.len());
    Ok(())
}

async fn run_search(
    query: &str,
    session: &str,
    field: &str,
    config: &config::Config,
) -> Result<()> {
    let db_path = config
        .data_dir
        .join("sessions")
        .join(format!("{session}.db"));
    let pool = db::init_db(&db_path).await?;
    let engine = search::SearchEngine::new(pool);

    let results = engine.search(query, field, session).await?;
    eprintln!(
        "[wireclaw] search '{query}' in field '{field}', session '{session}': {} results",
        results.len()
    );
    for exchange in &results {
        let status = exchange.status_label();
        println!(
            "  {} {} {} ({})",
            exchange.request.method, exchange.request.path, status, exchange.request.host,
        );
    }
    Ok(())
}

async fn run_export(
    format: cli::ExportFormat,
    session: &str,
    output: Option<&std::path::Path>,
    config: &config::Config,
) -> Result<()> {
    let db_path = config
        .data_dir
        .join("sessions")
        .join(format!("{session}.db"));
    let pool = db::init_db(&db_path).await?;
    let exporter = export::Exporter::new(pool);

    let content = exporter.export(format, session, output).await?;
    if output.is_none() {
        println!("{content}");
    }
    Ok(())
}

async fn run_tui(session: &str, config: &config::Config) -> Result<()> {
    let db_path = config
        .data_dir
        .join("sessions")
        .join(format!("{session}.db"));
    let pool = db::init_db(&db_path).await?;

    let mut app = tui::App::new(pool, session.to_string());
    app.run().await?;
    Ok(())
}

async fn run_ca(command: cli::CaCommands, config: &config::Config) -> Result<()> {
    let cert_dir = config.data_dir.join("certs");
    let mgr = cert::CertManager::load_or_create(&cert_dir)?;

    match command {
        cli::CaCommands::Generate => {
            eprintln!(
                "[wireclaw] CA certificate ready at {}",
                mgr.ca_cert_path().display()
            );
            eprintln!(
                "[wireclaw] Install this CA in your browser/system to trust intercepted HTTPS traffic:"
            );
            eprintln!();
            println!("{}", mgr.ca_cert_pem());
            eprintln!();
            eprintln!("[wireclaw] Trust instructions:");
            eprintln!(
                "  Linux (system-wide):  sudo cp {} /usr/local/share/ca-certificates/wireclaw.crt && sudo update-ca-certificates",
                mgr.ca_cert_path().display()
            );
            eprintln!(
                "  Linux (Firefox):      Settings → Privacy & Security → Certificates → View Certificates → Import"
            );
            eprintln!(
                "  macOS:                sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain {}",
                mgr.ca_cert_path().display()
            );
            eprintln!(
                "  Chrome (all platforms): Settings → Privacy and security → Security → Manage certificates → Authorities → Import"
            );
        }
        cli::CaCommands::Show => {
            println!("{}", mgr.ca_cert_pem());
        }
    }

    Ok(())
}

async fn run_stats(session: &str, config: &config::Config) -> Result<()> {
    let db_path = config
        .data_dir
        .join("sessions")
        .join(format!("{session}.db"));
    let pool = db::init_db(&db_path).await?;

    let stats = stats::compute_session_stats(&pool, session).await?;
    let formatted = stats::format_stats(&stats, session);
    println!("{formatted}");
    Ok(())
}

async fn run_init(config: &config::Config) -> Result<()> {
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("wireclaw")
        .join("config.toml");

    if config_path.exists() {
        eprintln!(
            "[wireclaw] config already exists at {}",
            config_path.display()
        );
        eprintln!("[wireclaw] delete it first if you want to regenerate");
        return Ok(());
    }

    std::fs::create_dir_all(
        config_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("config path has no parent directory"))?,
    )?;

    let config_toml = r#"# Wireclaw configuration file
# Generated by `wireclaw init`

# Address the proxy listens on
listen_addr = "127.0.0.1:8080"

# Directory for session databases and CA certificates
# Default: platform-specific data dir (e.g. ~/.local/share/wireclaw on Linux)
data_dir = "{data_dir}"

[session]
# Automatically create sessions on first capture
auto_create = true
# Default session name if none specified
default_name = "default"

[proxy]
# Proxy listen address (same as top-level listen_addr by default)
listen_addr = "127.0.0.1:8080"
# Timeout for upstream connections in seconds
timeout_secs = 30
# Maximum body size to capture in bytes (10 MB)
max_body_size = 10485760
# Capture request/response headers
capture_headers = true
# Capture request/response bodies
capture_bodies = true

[replay]
# Delay between replayed requests in milliseconds
delay_ms = 0
# Follow HTTP redirects when replaying
follow_redirects = true
# Maximum number of redirects to follow
max_redirects = 10
"#;

    let data_dir_str = config.data_dir.to_string_lossy().replace('\\', "/");
    let contents = config_toml.replace("{data_dir}", &data_dir_str);

    std::fs::write(&config_path, contents)?;

    eprintln!("[wireclaw] config written to {}", config_path.display());
    eprintln!(
        "[wireclaw] edit it to customize proxy settings, session defaults, and replay behavior"
    );

    Ok(())
}

async fn run_ws_replay(
    request_id: &str,
    session: &str,
    delay_ms: u64,
    config: &config::Config,
) -> Result<()> {
    let db_path = config
        .data_dir
        .join("sessions")
        .join(format!("{session}.db"));
    let pool = db::init_db(&db_path).await?;

    // Find the original request to get the host
    let exchange = db::get_exchange_by_request_id(&pool, request_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("request {} not found", request_id))?;

    let host = exchange.request.host;
    eprintln!(
        "[wireclaw] ws-replay: connecting to {} for request {}",
        host, request_id
    );

    // Load all WS frames for this request_id
    let frames = db::list_ws_frames(&pool, request_id).await?;
    if frames.is_empty() {
        anyhow::bail!("no WebSocket frames found for request {}", request_id);
    }

    eprintln!("[wireclaw] ws-replay: replaying {} frames", frames.len());
    crate::websocket::replay_websocket(&host, &frames, delay_ms).await?;
    eprintln!("[wireclaw] ws-replay: complete");
    Ok(())
}

async fn run_dashboard(session: &str, addr: &str, config: &config::Config) -> Result<()> {
    let db_path = config
        .data_dir
        .join("sessions")
        .join(format!("{session}.db"));
    let pool = db::init_db(&db_path).await?;

    eprintln!(
        "[wireclaw] starting dashboard for session '{}' on {}",
        session, addr
    );
    dashboard::run_dashboard(pool, session.to_string(), addr).await
}

async fn run_diff(a: &str, b: &str, session: &str, config: &config::Config) -> Result<()> {
    let db_path = config
        .data_dir
        .join("sessions")
        .join(format!("{session}.db"));
    let pool = db::init_db(&db_path).await?;

    let exchange_a = db::get_exchange_by_request_id(&pool, a)
        .await?
        .ok_or_else(|| anyhow::anyhow!("request {} not found", a))?;

    let exchange_b = db::get_exchange_by_request_id(&pool, b)
        .await?
        .ok_or_else(|| anyhow::anyhow!("request {} not found", b))?;

    let result = diff::compare_exchanges(&exchange_a, &exchange_b);
    println!("{}", diff::format_diff_terminal(&result));
    Ok(())
}

async fn run_openapi(
    session: &str,
    output: Option<&std::path::Path>,
    config: &config::Config,
) -> Result<()> {
    let db_path = config
        .data_dir
        .join("sessions")
        .join(format!("{session}.db"));
    let pool = db::init_db(&db_path).await?;

    let spec = openapi::generate_from_session(&pool, session).await?;
    let json = serde_json::to_string_pretty(&spec)?;

    if let Some(path) = output {
        std::fs::write(path, json)?;
        eprintln!("[wireclaw] OpenAPI spec written to {}", path.display());
    } else {
        println!("{}", json);
    }
    Ok(())
}
