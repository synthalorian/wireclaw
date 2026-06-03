//! Request/response logging to SQLite.

use anyhow::Result;
use sqlx::SqlitePool;

use crate::db;
use crate::models::{CapturedRequest, CapturedResponse, Exchange};
use crate::websocket::WsFrame;

pub struct Logger {
    pool: SqlitePool,
}

impl Logger {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn log_request(&self, request: &CapturedRequest) -> Result<()> {
        db::store_request(&self.pool, request).await
    }

    pub async fn log_response(&self, response: &CapturedResponse) -> Result<()> {
        db::store_response(&self.pool, response).await
    }

    pub async fn log_exchange(&self, exchange: &Exchange) -> Result<()> {
        self.log_request(&exchange.request).await?;
        if let Some(ref response) = exchange.response {
            self.log_response(response).await?;
        }
        Ok(())
    }

    pub async fn log_ws_frame(&self, frame: &WsFrame) -> Result<()> {
        db::store_ws_frame(&self.pool, frame).await
    }
}
