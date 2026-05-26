//! Export captured traffic to HAR, curl, or raw HTTP formats.

use anyhow::Result;
use sqlx::SqlitePool;
use std::path::Path;

use crate::cli::ExportFormat;
use crate::models::Exchange;

pub struct Exporter {
    pool: SqlitePool,
}

impl Exporter {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn export(
        &self,
        format: ExportFormat,
        session: &str,
        output: Option<&Path>,
    ) -> Result<String> {
        let exchanges = self.fetch_session_exchanges(session).await?;
        let content = match format {
            ExportFormat::Har => self.to_har(&exchanges)?,
            ExportFormat::Curl => self.to_curl(&exchanges)?,
            ExportFormat::Raw => self.to_raw(&exchanges)?,
        };

        if let Some(path) = output {
            std::fs::write(path, &content)?;
        }

        Ok(content)
    }

    async fn fetch_session_exchanges(&self, session: &str) -> Result<Vec<Exchange>> {
        todo!("Query DB for all exchanges in session '{session}'")
    }

    fn to_har(&self, exchanges: &[Exchange]) -> Result<String> {
        todo!(
            "Serialize {n} exchanges into HAR 1.2 JSON format",
            n = exchanges.len()
        )
    }

    fn to_curl(&self, exchanges: &[Exchange]) -> Result<String> {
        todo!(
            "Generate curl commands for {n} exchanges",
            n = exchanges.len()
        )
    }

    fn to_raw(&self, exchanges: &[Exchange]) -> Result<String> {
        todo!(
            "Format {n} exchanges as raw HTTP request/response pairs",
            n = exchanges.len()
        )
    }
}
