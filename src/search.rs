//! Search and filter module for captured traffic.

use anyhow::Result;
use regex::Regex;
use sqlx::SqlitePool;

use crate::models::{Exchange, Filter};

pub struct SearchEngine {
    pool: SqlitePool,
}

impl SearchEngine {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn search(&self, query: &str, field: &str, session: &str) -> Result<Vec<Exchange>> {
        todo!(
            "Build SQL query with LIKE/REGEXP on field='{field}', query='{query}', session='{session}'"
        )
    }

    pub async fn filter(&self, _filter: &Filter) -> Result<Vec<Exchange>> {
        todo!("Construct dynamic WHERE clause from Filter struct and execute query")
    }

    pub fn compile_pattern(pattern: &str) -> Result<Regex> {
        Regex::new(pattern).map_err(|e| anyhow::anyhow!("invalid regex '{pattern}': {e}"))
    }
}
