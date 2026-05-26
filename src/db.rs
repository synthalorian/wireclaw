//! SQLite storage for captured HTTP exchanges.

use anyhow::Result;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::Path;

use crate::models::{CapturedRequest, CapturedResponse, Exchange, Filter, Session};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sessions (
    name        TEXT PRIMARY KEY,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    request_count INTEGER NOT NULL DEFAULT 0,
    db_path     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS requests (
    id          TEXT PRIMARY KEY,
    method      TEXT NOT NULL,
    url         TEXT NOT NULL,
    path        TEXT NOT NULL,
    host        TEXT NOT NULL,
    headers     TEXT NOT NULL,
    body        BLOB,
    timestamp   TEXT NOT NULL,
    session     TEXT NOT NULL REFERENCES sessions(name)
);

CREATE TABLE IF NOT EXISTS responses (
    id          TEXT PRIMARY KEY,
    request_id  TEXT NOT NULL UNIQUE REFERENCES requests(id),
    status      INTEGER NOT NULL,
    status_text TEXT NOT NULL,
    headers     TEXT NOT NULL,
    body        BLOB,
    timestamp   TEXT NOT NULL,
    latency_ms  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_requests_session ON requests(session);
CREATE INDEX IF NOT EXISTS idx_requests_method ON requests(method);
CREATE INDEX IF NOT EXISTS idx_requests_host ON requests(host);
CREATE INDEX IF NOT EXISTS idx_requests_timestamp ON requests(timestamp);
CREATE INDEX IF NOT EXISTS idx_responses_request_id ON responses(request_id);
"#;

pub async fn init_db(db_path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    sqlx::query(SCHEMA).execute(&pool).await?;
    Ok(pool)
}

pub async fn store_request(pool: &SqlitePool, request: &CapturedRequest) -> Result<()> {
    let headers_json = serde_json::to_string(&request.headers)?;
    let body = request.body.as_deref();

    sqlx::query(
        "INSERT INTO requests (id, method, url, path, host, headers, body, timestamp, session)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&request.id)
    .bind(&request.method)
    .bind(&request.url)
    .bind(&request.path)
    .bind(&request.host)
    .bind(&headers_json)
    .bind(body)
    .bind(request.timestamp.to_rfc3339())
    .bind(&request.session)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn store_response(pool: &SqlitePool, response: &CapturedResponse) -> Result<()> {
    let headers_json = serde_json::to_string(&response.headers)?;
    let body = response.body.as_deref();

    sqlx::query(
        "INSERT INTO responses (id, request_id, status, status_text, headers, body, timestamp, latency_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&response.id)
    .bind(&response.request_id)
    .bind(response.status)
    .bind(&response.status_text)
    .bind(&headers_json)
    .bind(body)
    .bind(response.timestamp.to_rfc3339())
    .bind(response.latency_ms as i64)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn list_exchanges(
    _pool: &SqlitePool,
    session: &str,
    limit: usize,
) -> Result<Vec<Exchange>> {
    todo!(
        "Implement exchange listing with request/response join query for session={session}, limit={limit}"
    )
}

pub async fn search_exchanges(_pool: &SqlitePool, _filter: &Filter) -> Result<Vec<Exchange>> {
    todo!("Implement filtered search with dynamic WHERE clause construction")
}

pub async fn get_session(_pool: &SqlitePool, name: &str) -> Result<Option<Session>> {
    todo!("Implement session lookup by name={name}")
}

pub async fn create_session(pool: &SqlitePool, session: &Session) -> Result<()> {
    let db_path = &session.db_path;
    sqlx::query(
        "INSERT INTO sessions (name, created_at, updated_at, request_count, db_path)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&session.name)
    .bind(session.created_at.to_rfc3339())
    .bind(session.updated_at.to_rfc3339())
    .bind(session.request_count as i64)
    .bind(db_path)
    .execute(pool)
    .await?;
    Ok(())
}
