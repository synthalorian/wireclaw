mod cert;
mod chain;
mod cli;
mod config;
mod db;
mod export;
mod intercept;
mod logger;
mod models;
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
    let cli = cli::Cli::parse();
    let config = config::load_config(&cli.config)?;

    match cli.command {
        cli::Commands::Capture {
            session,
            verbose,
            intercept,
            intercept_rule,
        } => {
            run_capture(
                &cli.addr,
                &session,
                verbose,
                intercept,
                intercept_rule,
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
    }
}

async fn run_capture(
    addr: &str,
    session: &str,
    verbose: bool,
    intercept: bool,
    intercept_rule: Option<String>,
    config: &config::Config,
) -> Result<()> {
    eprintln!(
        "[ledger] starting capture on {addr}, session={session}, verbose={verbose}, intercept={intercept}"
    );
    let data_dir = config.data_dir.join("sessions");
    let db_path = data_dir.join(format!("{session}.db"));
    let pool = db::init_db(&db_path).await?;

    let session_model = models::Session::new(session.to_string(), db_path.display().to_string());
    db::create_session(&pool, &session_model).await?;

    let listen_addr = proxy::parse_addr(addr)?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<models::Exchange>(256);
    let logger = logger::Logger::new(pool.clone());

    let cert_dir = config.data_dir.join("certs");
    let cert_mgr = Arc::new(cert::CertManager::load_or_create(&cert_dir)?);

    // Build intercept rules if enabled
    let intercept_rules = if intercept {
        let mut rules = Vec::new();
        if let Some(ref expr) = intercept_rule {
            match crate::intercept::InterceptRule::parse(expr) {
                Ok(rule) => rules.push(rule),
                Err(e) => eprintln!("[ledger] warning: invalid intercept rule: {e}"),
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
        session.to_string(),
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
                    .get("x-ledger-ws-direction")
                    .map(|s| s.as_str())
                    .unwrap_or("client->server");
                let opcode = exchange
                    .request
                    .headers
                    .get("x-ledger-ws-opcode")
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
                    eprintln!("[ledger] failed to log ws frame: {e}");
                }
                continue;
            }
            if let Err(e) = logger.log_exchange(&exchange).await {
                eprintln!("[ledger] failed to log exchange: {e}");
            }
            if verbose {
                eprintln!(
                    "[ledger] {} {} {} ({})",
                    exchange.request.method,
                    exchange.request.path,
                    exchange.status_label(),
                    exchange.request.host,
                );
            }
        }
    });

    tokio::select! {
        r = proxy_handle => r??,
        r = logger_handle => r?,
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
        eprintln!("[ledger] chain complete. extracted variables:");
        for (k, v) in &vars {
            eprintln!("  ${{{k}}} = {v}");
        }
        return Ok(());
    }

    let engine = replay::ReplayEngine::new(pool);

    match (args.id, args.filter) {
        (Some(request_id), _) => {
            eprintln!(
                "[ledger] replaying request {request_id} x{} (dry_run={}, diff={}, edit={})",
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
                "[ledger] replaying filtered requests: {filter_expr} (dry_run={}, diff={})",
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
        "[ledger] session={session}, showing {limit} exchanges (headers={headers}, bodies={bodies})"
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
    eprintln!("[ledger] {} exchanges", exchanges.len());
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
        "[ledger] search '{query}' in field '{field}', session '{session}': {} results",
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
                "[ledger] CA certificate ready at {}",
                mgr.ca_cert_path().display()
            );
            eprintln!(
                "[ledger] Install this CA in your browser/system to trust intercepted HTTPS traffic:"
            );
            eprintln!();
            println!("{}", mgr.ca_cert_pem());
            eprintln!();
            eprintln!("[ledger] Trust instructions:");
            eprintln!(
                "  Linux (system-wide):  sudo cp {} /usr/local/share/ca-certificates/ledger.crt && sudo update-ca-certificates",
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
        .join("ledger")
        .join("config.toml");

    if config_path.exists() {
        eprintln!(
            "[ledger] config already exists at {}",
            config_path.display()
        );
        eprintln!("[ledger] delete it first if you want to regenerate");
        return Ok(());
    }

    std::fs::create_dir_all(
        config_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("config path has no parent directory"))?,
    )?;

    let config_toml = r#"# Ledger configuration file
# Generated by `ledger init`

# Address the proxy listens on
listen_addr = "127.0.0.1:8080"

# Directory for session databases and CA certificates
# Default: platform-specific data dir (e.g. ~/.local/share/ledger on Linux)
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

    eprintln!("[ledger] config written to {}", config_path.display());
    eprintln!(
        "[ledger] edit it to customize proxy settings, session defaults, and replay behavior"
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
        "[ledger] ws-replay: connecting to {} for request {}",
        host, request_id
    );

    // Load all WS frames for this request_id
    let frames = db::list_ws_frames(&pool, request_id).await?;
    if frames.is_empty() {
        anyhow::bail!("no WebSocket frames found for request {}", request_id);
    }

    eprintln!("[ledger] ws-replay: replaying {} frames", frames.len());
    crate::websocket::replay_websocket(&host, &frames, delay_ms).await?;
    eprintln!("[ledger] ws-replay: complete");
    Ok(())
}
