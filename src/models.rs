//! Data models for captured HTTP traffic.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedRequest {
    pub id: String,
    pub method: String,
    pub url: String,
    pub path: String,
    pub host: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub timestamp: DateTime<Utc>,
    pub session: String,
}

impl CapturedRequest {
    pub fn new(method: String, url: String, host: String, session: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            path: url.clone(),
            method,
            url,
            host,
            headers: HashMap::new(),
            body: None,
            timestamp: Utc::now(),
            session,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedResponse {
    pub id: String,
    pub request_id: String,
    pub status: u16,
    pub status_text: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
    pub timestamp: DateTime<Utc>,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exchange {
    pub request: CapturedRequest,
    pub response: Option<CapturedResponse>,
}

impl Exchange {
    pub fn status_label(&self) -> &str {
        match self.response {
            Some(ref r) if r.status >= 200 && r.status < 300 => "2xx",
            Some(ref r) if r.status >= 300 && r.status < 400 => "3xx",
            Some(ref r) if r.status >= 400 && r.status < 500 => "4xx",
            Some(ref r) if r.status >= 500 => "5xx",
            Some(_) => "???",
            None => "---",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    pub method: Option<String>,
    pub path_pattern: Option<String>,
    pub status_code: Option<u16>,
    pub host: Option<String>,
    pub header_match: Option<(String, String)>,
    pub body_contains: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            method: None,
            path_pattern: None,
            status_code: None,
            host: None,
            header_match: None,
            body_contains: None,
            since: None,
            until: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub request_count: u64,
    pub db_path: String,
}

impl Session {
    pub fn new(name: String, db_path: String) -> Self {
        let now = Utc::now();
        Self {
            name,
            created_at: now,
            updated_at: now,
            request_count: 0,
            db_path,
        }
    }
}
