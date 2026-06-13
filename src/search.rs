//! Search and filter module for captured traffic.

use anyhow::Result;
use sqlx::SqlitePool;

use crate::db;
use crate::models::{Exchange, Filter};

pub struct SearchEngine {
    pool: SqlitePool,
}

impl SearchEngine {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn search(&self, query: &str, field: &str, session: &str) -> Result<Vec<Exchange>> {
        let mut filter = Filter {
            session: Some(session.to_string()),
            ..Filter::default()
        };

        match field.to_lowercase().as_str() {
            "method" => filter.method = Some(query.to_uppercase()),
            "path" => filter.path_pattern = Some(query.to_string()),
            "host" => filter.host = Some(query.to_string()),
            "status" => {
                filter.status_code = Some(
                    query
                        .parse()
                        .map_err(|e| anyhow::anyhow!("invalid status code: {e}"))?,
                )
            }
            "body" => filter.body_contains = Some(query.to_string()),
            _ => {
                // Default: search across path, host, and body
                filter.path_pattern = Some(query.to_string());
            }
        }

        db::search_exchanges(&self.pool, &filter).await
    }
}
