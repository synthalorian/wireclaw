//! Request/response logging to SQLite.

use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::dashboard::{DashboardEvent, broadcast_exchange};
use crate::db;
use crate::models::{CapturedRequest, CapturedResponse, Exchange};
use crate::websocket::WsFrame;

pub struct Logger {
    pool: SqlitePool,
    tx: Option<broadcast::Sender<DashboardEvent>>,
}

impl Logger {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool, tx: None }
    }

    pub fn with_broadcast(mut self, tx: broadcast::Sender<DashboardEvent>) -> Self {
        self.tx = Some(tx);
        self
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
        if let Some(ref tx) = self.tx {
            broadcast_exchange(tx, exchange);
        }
        Ok(())
    }

    pub async fn log_ws_frame(&self, frame: &WsFrame) -> Result<()> {
        db::store_ws_frame(&self.pool, frame).await
    }
}
