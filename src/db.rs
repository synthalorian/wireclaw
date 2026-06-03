//! SQLite storage for captured HTTP exchanges.

use anyhow::Result;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::Path;

use crate::models::{CapturedRequest, CapturedResponse, Exchange, Filter, Session};
use crate::websocket::{WsDirection, WsFrame};

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

CREATE TABLE IF NOT EXISTS ws_frames (
    id          TEXT PRIMARY KEY,
    request_id  TEXT NOT NULL REFERENCES requests(id),
    direction   TEXT NOT NULL,
    opcode      TEXT NOT NULL,
    payload     BLOB,
    timestamp   TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_requests_session ON requests(session);
CREATE INDEX IF NOT EXISTS idx_requests_method ON requests(method);
CREATE INDEX IF NOT EXISTS idx_requests_host ON requests(host);
CREATE INDEX IF NOT EXISTS idx_requests_timestamp ON requests(timestamp);
CREATE INDEX IF NOT EXISTS idx_responses_request_id ON responses(request_id);
CREATE INDEX IF NOT EXISTS idx_ws_frames_request_id ON ws_frames(request_id);
"#;

pub async fn init_db(db_path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .foreign_keys(true);

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
    .bind(response.status as i64)
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
    pool: &SqlitePool,
    session: &str,
    limit: usize,
) -> Result<Vec<Exchange>> {
    let rows: Vec<ExchangeRow> = sqlx::query_as(
        r#"
        SELECT
            r.id as req_id, r.method, r.url, r.path, r.host, r.headers as req_headers, r.body as req_body,
            r.timestamp as req_timestamp, r.session,
            s.id as resp_id, s.request_id, s.status, s.status_text, s.headers as resp_headers,
            s.body as resp_body, s.timestamp as resp_timestamp, s.latency_ms
        FROM requests r
        LEFT JOIN responses s ON r.id = s.request_id
        WHERE r.session = ?
        ORDER BY r.timestamp DESC
        LIMIT ?
        "#,
    )
    .bind(session)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(|r| r.into_exchange()).collect::<Result<Vec<_>>>()
}

pub async fn search_exchanges(pool: &SqlitePool, filter: &Filter) -> Result<Vec<Exchange>> {
    let mut conditions = vec!["1=1".to_string()];
    let mut params: Vec<String> = Vec::new();

    if let Some(ref session) = filter.session {
        conditions.push("r.session = ?".to_string());
        params.push(session.clone());
    }
    if let Some(ref method) = filter.method {
        conditions.push("r.method = ?".to_string());
        params.push(method.clone());
    }
    if let Some(ref host) = filter.host {
        conditions.push("r.host LIKE ?".to_string());
        params.push(format!("%{}%", host));
    }
    if let Some(ref path_pattern) = filter.path_pattern {
        conditions.push("r.path LIKE ?".to_string());
        params.push(format!("%{}%", path_pattern));
    }
    if let Some(status_code) = filter.status_code {
        conditions.push("s.status = ?".to_string());
        params.push(status_code.to_string());
    }
    if let Some(ref body_contains) = filter.body_contains {
        conditions.push("(r.body LIKE ? OR s.body LIKE ?)".to_string());
        params.push(format!("%{}%", body_contains));
        params.push(format!("%{}%", body_contains));
    }

    let where_clause = conditions.join(" AND ");
    let sql = format!(
        r#"
        SELECT
            r.id as req_id, r.method, r.url, r.path, r.host, r.headers as req_headers, r.body as req_body,
            r.timestamp as req_timestamp, r.session,
            s.id as resp_id, s.request_id, s.status, s.status_text, s.headers as resp_headers,
            s.body as resp_body, s.timestamp as resp_timestamp, s.latency_ms
        FROM requests r
        LEFT JOIN responses s ON r.id = s.request_id
        WHERE {}
        ORDER BY r.timestamp DESC
        "#,
        where_clause
    );

    let mut query = sqlx::query_as::<_, ExchangeRow>(&sql);
    for param in &params {
        query = query.bind(param);
    }

    let rows = query.fetch_all(pool).await?;
    rows.into_iter().map(|r| r.into_exchange()).collect::<Result<Vec<_>>>()
}

pub async fn get_request_by_id(pool: &SqlitePool, request_id: &str) -> Result<Option<Exchange>> {
    let row: Option<ExchangeRow> = sqlx::query_as(
        r#"
        SELECT
            r.id as req_id, r.method, r.url, r.path, r.host, r.headers as req_headers, r.body as req_body,
            r.timestamp as req_timestamp, r.session,
            s.id as resp_id, s.request_id, s.status, s.status_text, s.headers as resp_headers,
            s.body as resp_body, s.timestamp as resp_timestamp, s.latency_ms
        FROM requests r
        LEFT JOIN responses s ON r.id = s.request_id
        WHERE r.id = ?
        "#,
    )
    .bind(request_id)
    .fetch_optional(pool)
    .await?;

    match row {
        Some(r) => Ok(Some(r.into_exchange()?)),
        None => Ok(None),
    }
}

#[allow(dead_code)]
pub async fn get_session(pool: &SqlitePool, name: &str) -> Result<Option<Session>> {
    let row: Option<SessionRow> = sqlx::query_as(
        "SELECT name, created_at, updated_at, request_count, db_path FROM sessions WHERE name = ?"
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;

    match row {
        Some(r) => Ok(Some(r.into_session()?)),
        None => Ok(None),
    }
}

pub async fn create_session(pool: &SqlitePool, session: &Session) -> Result<()> {
    let db_path = &session.db_path;
    sqlx::query(
        "INSERT INTO sessions (name, created_at, updated_at, request_count, db_path)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(name) DO UPDATE SET
            updated_at = excluded.updated_at,
            request_count = request_count + 1"
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

pub async fn store_ws_frame(pool: &SqlitePool, frame: &WsFrame) -> Result<()> {
    let direction = match frame.direction {
        WsDirection::ClientToServer => "c2s",
        WsDirection::ServerToClient => "s2c",
    };
    let payload = frame.payload.as_deref();

    sqlx::query(
        "INSERT INTO ws_frames (id, request_id, direction, opcode, payload, timestamp)
         VALUES (?, ?, ?, ?, ?, ?)"
    )
    .bind(&frame.id)
    .bind(&frame.request_id)
    .bind(direction)
    .bind(&frame.opcode)
    .bind(payload)
    .bind(frame.timestamp.to_rfc3339())
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn list_ws_frames(pool: &SqlitePool, request_id: &str) -> Result<Vec<WsFrame>> {
    let rows: Vec<WsFrameRow> = sqlx::query_as(
        "SELECT id, request_id, direction, opcode, payload, timestamp FROM ws_frames WHERE request_id = ? ORDER BY timestamp"
    )
    .bind(request_id)
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(|r| r.into_ws_frame()).collect::<Result<Vec<_>>>()
}

pub async fn get_exchange_by_request_id(pool: &SqlitePool, request_id: &str) -> Result<Option<Exchange>> {
    get_request_by_id(pool, request_id).await
}

// ── Row structs for sqlx::query_as ──────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct WsFrameRow {
    id: String,
    request_id: String,
    direction: String,
    opcode: String,
    payload: Option<Vec<u8>>,
    timestamp: String,
}

impl WsFrameRow {
    fn into_ws_frame(self) -> Result<WsFrame> {
        let direction = match self.direction.as_str() {
            "s2c" => WsDirection::ServerToClient,
            _ => WsDirection::ClientToServer,
        };
        Ok(WsFrame {
            id: self.id,
            request_id: self.request_id,
            direction,
            opcode: self.opcode,
            payload: self.payload,
            timestamp: chrono::DateTime::parse_from_rfc3339(&self.timestamp)
                .map_err(|e| anyhow::anyhow!("parse timestamp: {e}"))?
                .with_timezone(&chrono::Utc),
        })
    }
}

#[derive(sqlx::FromRow)]
struct ExchangeRow {
    req_id: String,
    method: String,
    url: String,
    path: String,
    host: String,
    req_headers: String,
    req_body: Option<Vec<u8>>,
    req_timestamp: String,
    session: String,
    resp_id: Option<String>,
    request_id: Option<String>,
    status: Option<i64>,
    status_text: Option<String>,
    resp_headers: Option<String>,
    resp_body: Option<Vec<u8>>,
    resp_timestamp: Option<String>,
    latency_ms: Option<i64>,
}

impl ExchangeRow {
    fn into_exchange(self) -> Result<Exchange> {
        let req_headers: std::collections::HashMap<String, String> =
            serde_json::from_str(&self.req_headers)?;

        let request = CapturedRequest {
            id: self.req_id,
            method: self.method,
            url: self.url,
            path: self.path,
            host: self.host,
            headers: req_headers,
            body: self.req_body,
            timestamp: chrono::DateTime::parse_from_rfc3339(&self.req_timestamp)
                .map_err(|e| anyhow::anyhow!("parse timestamp: {e}"))?
                .with_timezone(&chrono::Utc),
            session: self.session,
        };

        let response = if let Some(resp_id) = self.resp_id {
            let resp_headers: std::collections::HashMap<String, String> =
                serde_json::from_str(&self.resp_headers.unwrap_or_default())?;

            Some(CapturedResponse {
                id: resp_id,
                request_id: self.request_id.unwrap_or_default(),
                status: self.status.unwrap_or(0) as u16,
                status_text: self.status_text.unwrap_or_default(),
                headers: resp_headers,
                body: self.resp_body,
                timestamp: chrono::DateTime::parse_from_rfc3339(
                    &self.resp_timestamp.unwrap_or_default(),
                )
                .map_err(|e| anyhow::anyhow!("parse timestamp: {e}"))?
                .with_timezone(&chrono::Utc),
                latency_ms: self.latency_ms.unwrap_or(0) as u64,
            })
        } else {
            None
        };

        Ok(Exchange { request, response })
    }
}

#[derive(sqlx::FromRow)]
#[allow(dead_code)]
struct SessionRow {
    name: String,
    created_at: String,
    updated_at: String,
    request_count: i64,
    db_path: String,
}

impl SessionRow {
    #[allow(dead_code)]
    fn into_session(self) -> Result<Session> {
        Ok(Session {
            name: self.name,
            created_at: chrono::DateTime::parse_from_rfc3339(&self.created_at)
                .map_err(|e| anyhow::anyhow!("parse timestamp: {e}"))?
                .with_timezone(&chrono::Utc),
            updated_at: chrono::DateTime::parse_from_rfc3339(&self.updated_at)
                .map_err(|e| anyhow::anyhow!("parse timestamp: {e}"))?
                .with_timezone(&chrono::Utc),
            request_count: self.request_count as u64,
            db_path: self.db_path,
        })
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    async fn setup_test_db() -> (SqlitePool, TempDir) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = init_db(&db_path).await.unwrap();
        (pool, dir)
    }

    fn make_request(id: &str, method: &str, url: &str, host: &str, session: &str) -> CapturedRequest {
        let mut headers = HashMap::new();
        headers.insert("host".to_string(), host.to_string());
        CapturedRequest {
            id: id.to_string(),
            method: method.to_string(),
            url: url.to_string(),
            path: "/test".to_string(),
            host: host.to_string(),
            headers,
            body: Some(b"test body".to_vec()),
            timestamp: chrono::Utc::now(),
            session: session.to_string(),
        }
    }

    fn make_response(request_id: &str, status: u16) -> CapturedResponse {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        CapturedResponse {
            id: "resp-1".to_string(),
            request_id: request_id.to_string(),
            status,
            status_text: "OK".to_string(),
            headers,
            body: Some(b"{\"ok\": true}".to_vec()),
            timestamp: chrono::Utc::now(),
            latency_ms: 42,
        }
    }

    #[tokio::test]
    async fn test_init_db_creates_tables() {
        let (pool, _dir) = setup_test_db().await;
        // list_exchanges should return empty vec, not error
        let exchanges = list_exchanges(&pool, "test", 10).await.unwrap();
        assert!(exchanges.is_empty());
    }

    #[tokio::test]
    async fn test_store_and_list_request() {
        let (pool, _dir) = setup_test_db().await;

        let session = Session::new("test".to_string(), "/tmp/test.db".to_string());
        create_session(&pool, &session).await.unwrap();

        let req = make_request("req-1", "GET", "http://example.com/test", "example.com", "test");
        store_request(&pool, &req).await.unwrap();

        let exchanges = list_exchanges(&pool, "test", 10).await.unwrap();
        assert_eq!(exchanges.len(), 1);
        assert_eq!(exchanges[0].request.id, "req-1");
        assert_eq!(exchanges[0].request.method, "GET");
        assert!(exchanges[0].response.is_none());
    }

    #[tokio::test]
    async fn test_store_request_and_response() {
        let (pool, _dir) = setup_test_db().await;

        let session = Session::new("test".to_string(), "/tmp/test.db".to_string());
        create_session(&pool, &session).await.unwrap();

        let req = make_request("req-2", "POST", "http://api.example.com/users", "api.example.com", "test");
        store_request(&pool, &req).await.unwrap();

        let resp = make_response("req-2", 201);
        store_response(&pool, &resp).await.unwrap();

        let exchanges = list_exchanges(&pool, "test", 10).await.unwrap();
        assert_eq!(exchanges.len(), 1);
        assert_eq!(exchanges[0].request.id, "req-2");
        assert_eq!(exchanges[0].request.method, "POST");

        let response = exchanges[0].response.as_ref().unwrap();
        assert_eq!(response.status, 201);
        assert_eq!(response.latency_ms, 42);
    }

    #[tokio::test]
    async fn test_get_request_by_id() {
        let (pool, _dir) = setup_test_db().await;

        let session = Session::new("test".to_string(), "/tmp/test.db".to_string());
        create_session(&pool, &session).await.unwrap();

        let req = make_request("req-3", "DELETE", "http://example.com/item", "example.com", "test");
        store_request(&pool, &req).await.unwrap();

        let found = get_request_by_id(&pool, "req-3").await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().request.method, "DELETE");

        let not_found = get_request_by_id(&pool, "nonexistent").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_search_by_method() {
        let (pool, _dir) = setup_test_db().await;

        let session = Session::new("test".to_string(), "/tmp/test.db".to_string());
        create_session(&pool, &session).await.unwrap();

        let req1 = make_request("req-a", "GET", "http://example.com/a", "example.com", "test");
        let req2 = make_request("req-b", "POST", "http://example.com/b", "example.com", "test");
        store_request(&pool, &req1).await.unwrap();
        store_request(&pool, &req2).await.unwrap();

        let mut filter = Filter::default();
        filter.session = Some("test".to_string());
        filter.method = Some("POST".to_string());

        let results = search_exchanges(&pool, &filter).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].request.method, "POST");
    }

    #[tokio::test]
    async fn test_list_respects_limit() {
        let (pool, _dir) = setup_test_db().await;

        let session = Session::new("test".to_string(), "/tmp/test.db".to_string());
        create_session(&pool, &session).await.unwrap();

        for i in 0..10 {
            let req = make_request(
                &format!("req-{}", i),
                "GET",
                &format!("http://example.com/{}", i),
                "example.com",
                "test",
            );
            store_request(&pool, &req).await.unwrap();
        }

        let exchanges = list_exchanges(&pool, "test", 5).await.unwrap();
        assert_eq!(exchanges.len(), 5);
    }
}
