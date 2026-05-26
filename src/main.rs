mod cli;
mod config;
mod db;
mod error;
mod export;
mod logger;
mod models;
mod proxy;
mod replay;
mod search;
mod tui;

use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    let config = config::load_config(&cli.config)?;

    match cli.command {
        cli::Commands::Capture { session, verbose } => {
            run_capture(&cli.addr, &session, verbose, &config).await
        }
        cli::Commands::Replay {
            id,
            count,
            dry_run,
            filter,
        } => run_replay(id, count, dry_run, filter, &config).await,
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
    }
}

async fn run_capture(
    addr: &str,
    session: &str,
    verbose: bool,
    config: &config::Config,
) -> Result<()> {
    eprintln!("[ledger] starting capture on {addr}, session={session}, verbose={verbose}");
    let data_dir = config.data_dir.join("sessions");
    let db_path = data_dir.join(format!("{session}.db"));
    let pool = db::init_db(&db_path).await?;

    let session_model = models::Session::new(session.to_string(), db_path.display().to_string());
    db::create_session(&pool, &session_model).await?;

    let listen_addr = proxy::parse_addr(addr)?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<models::Exchange>(256);
    let logger = logger::Logger::new(pool.clone());

    let proxy = std::sync::Arc::new(proxy::ProxyServer::new(listen_addr, tx));
    let proxy_handle = tokio::spawn(async move { proxy.run().await });

    let logger_handle = tokio::spawn(async move {
        while let Some(exchange) = rx.recv().await {
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

async fn run_replay(
    id: Option<String>,
    count: u32,
    dry_run: bool,
    filter: Option<String>,
    config: &config::Config,
) -> Result<()> {
    let db_path = config.data_dir.join("sessions").join("default.db");
    let pool = db::init_db(&db_path).await?;
    let engine = replay::ReplayEngine::new(pool);

    match (id, filter) {
        (Some(request_id), _) => {
            eprintln!("[ledger] replaying request {request_id} x{count} (dry_run={dry_run})");
            engine.replay_by_id(&request_id, count, dry_run).await?;
        }
        (None, Some(filter_expr)) => {
            eprintln!("[ledger] replaying filtered requests: {filter_expr} (dry_run={dry_run})");
            engine.replay_filtered(&filter_expr, dry_run).await?;
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
        eprintln!(
            "  {} {} {} ({})",
            exchange.request.method, exchange.request.path, status, exchange.request.host,
        );
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
