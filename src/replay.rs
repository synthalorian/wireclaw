//! Request replay engine.

use anyhow::Result;
use sqlx::SqlitePool;

use crate::models::{CapturedRequest, Exchange};

pub struct ReplayEngine {
    pool: SqlitePool,
}

impl ReplayEngine {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn replay_by_id(&self, request_id: &str, count: u32, dry_run: bool) -> Result<()> {
        todo!(
            "Fetch request {request_id} from DB, replay it {count} times (dry_run={dry_run}) via hyper client"
        )
    }

    pub async fn replay_filtered(&self, filter_expr: &str, dry_run: bool) -> Result<()> {
        todo!(
            "Parse filter expression '{filter_expr}', find matching requests, replay each (dry_run={dry_run})"
        )
    }

    pub async fn build_http_request(
        &self,
        _captured: &CapturedRequest,
    ) -> Result<hyper::Request<String>> {
        todo!("Convert CapturedRequest into a live hyper::Request, reconstructing headers and body")
    }

    pub async fn execute_replay(&self, _request: hyper::Request<String>) -> Result<Exchange> {
        todo!(
            "Send the reconstructed request via hyper client, capture the response, return Exchange"
        )
    }
}
