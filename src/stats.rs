//! Session statistics and metrics.

use anyhow::Result;
use sqlx::SqlitePool;
use std::collections::HashMap;

use crate::db;

/// Aggregated statistics for a session.
#[derive(Debug, Clone)]
pub struct SessionStats {
    pub total_requests: usize,
    pub avg_latency_ms: u64,
    pub error_rate_percent: f64,
    pub status_breakdown: HashMap<u16, usize>,
    pub top_endpoints: Vec<(String, usize)>,
    pub latency_histogram: Vec<(String, usize)>,
    pub method_breakdown: HashMap<String, usize>,
    pub host_breakdown: HashMap<String, usize>,
}

impl SessionStats {
    pub fn empty() -> Self {
        Self {
            total_requests: 0,
            avg_latency_ms: 0,
            error_rate_percent: 0.0,
            status_breakdown: HashMap::new(),
            top_endpoints: Vec::new(),
            latency_histogram: Vec::new(),
            method_breakdown: HashMap::new(),
            host_breakdown: HashMap::new(),
        }
    }
}

/// Compute statistics for a session.
pub async fn compute_session_stats(pool: &SqlitePool, session: &str) -> Result<SessionStats> {
    let exchanges = db::list_exchanges(pool, session, 100_000).await?;

    if exchanges.is_empty() {
        return Ok(SessionStats::empty());
    }

    let total = exchanges.len();

    // Latency
    let latencies: Vec<u64> = exchanges
        .iter()
        .filter_map(|ex| ex.response.as_ref().map(|r| r.latency_ms))
        .collect();

    let avg_latency = if !latencies.is_empty() {
        latencies.iter().sum::<u64>() / latencies.len() as u64
    } else {
        0
    };

    // Error rate (4xx + 5xx)
    let error_count = exchanges
        .iter()
        .filter(|ex| matches!(ex.response, Some(ref r) if r.status >= 400))
        .count();
    let error_rate = (error_count as f64 / total as f64) * 100.0;

    // Status breakdown
    let mut status_breakdown: HashMap<u16, usize> = HashMap::new();
    for ex in &exchanges {
        if let Some(ref r) = ex.response {
            *status_breakdown.entry(r.status).or_insert(0) += 1;
        }
    }

    // Method breakdown
    let mut method_breakdown: HashMap<String, usize> = HashMap::new();
    for ex in &exchanges {
        *method_breakdown
            .entry(ex.request.method.clone())
            .or_insert(0) += 1;
    }

    // Host breakdown
    let mut host_breakdown: HashMap<String, usize> = HashMap::new();
    for ex in &exchanges {
        *host_breakdown
            .entry(ex.request.host.clone())
            .or_insert(0) += 1;
    }

    // Top endpoints by hit count
    let mut endpoint_counts: HashMap<String, usize> = HashMap::new();
    for ex in &exchanges {
        let key = format!("{} {}", ex.request.method, ex.request.path);
        *endpoint_counts.entry(key).or_insert(0) += 1;
    }

    let mut top_endpoints: Vec<(String, usize)> = endpoint_counts.into_iter().collect();
    top_endpoints.sort_by(|a, b| b.1.cmp(&a.1));
    top_endpoints.truncate(10);

    // Latency histogram (buckets in ms)
    let mut latency_histogram: Vec<(String, usize)> = vec![
        ("0-50".to_string(), 0),
        ("50-100".to_string(), 0),
        ("100-250".to_string(), 0),
        ("250-500".to_string(), 0),
        ("500-1000".to_string(), 0),
        ("1000-2500".to_string(), 0),
        ("2500+".to_string(), 0),
    ];

    for lat in &latencies {
        let idx = match *lat {
            0..=50 => 0,
            51..=100 => 1,
            101..=250 => 2,
            251..=500 => 3,
            501..=1000 => 4,
            1001..=2500 => 5,
            _ => 6,
        };
        latency_histogram[idx].1 += 1;
    }

    Ok(SessionStats {
        total_requests: total,
        avg_latency_ms: avg_latency,
        error_rate_percent: error_rate,
        status_breakdown,
        top_endpoints,
        latency_histogram,
        method_breakdown,
        host_breakdown,
    })
}

/// Format stats as human-readable text.
pub fn format_stats(stats: &SessionStats, session: &str) -> String {
    let mut lines = Vec::new();

    lines.push(format!("Session: {session}"));
    lines.push("─".repeat(50));
    lines.push(format!("Total requests: {}", stats.total_requests));
    lines.push(format!("Average latency: {} ms", stats.avg_latency_ms));
    lines.push(format!("Error rate: {:.1}%", stats.error_rate_percent));
    lines.push(String::new());

    if !stats.method_breakdown.is_empty() {
        lines.push("Methods:".to_string());
        let mut methods: Vec<_> = stats.method_breakdown.iter().collect();
        methods.sort_by(|a, b| b.1.cmp(a.1));
        for (method, count) in methods {
            lines.push(format!("  {method:>6}: {count}"));
        }
        lines.push(String::new());
    }

    if !stats.status_breakdown.is_empty() {
        lines.push("Status codes:".to_string());
        let mut statuses: Vec<_> = stats.status_breakdown.iter().collect();
        statuses.sort_by_key(|(s, _)| *s);
        for (status, count) in statuses {
            lines.push(format!("  {status:>3}: {count}"));
        }
        lines.push(String::new());
    }

    if !stats.host_breakdown.is_empty() {
        lines.push("Hosts:".to_string());
        let mut hosts: Vec<_> = stats.host_breakdown.iter().collect();
        hosts.sort_by(|a, b| b.1.cmp(a.1));
        for (host, count) in hosts {
            lines.push(format!("  {host}: {count}"));
        }
        lines.push(String::new());
    }

    if !stats.top_endpoints.is_empty() {
        lines.push("Top endpoints:".to_string());
        for (endpoint, count) in &stats.top_endpoints {
            lines.push(format!("  {endpoint}: {count}"));
        }
        lines.push(String::new());
    }

    if stats.latency_histogram.iter().any(|(_, c)| *c > 0) {
        lines.push("Latency distribution:".to_string());
        for (bucket, count) in &stats.latency_histogram {
            if *count > 0 {
                let bar = "█".repeat((*count).min(40));
                lines.push(format!("  {bucket:>10} ms: {bar} {count}"));
            }
        }
    }

    lines.join("\n")
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CapturedRequest, CapturedResponse, Exchange};
    use std::collections::HashMap;

    fn make_exchange(method: &str, path: &str, host: &str, status: u16, latency_ms: u64) -> Exchange {
        let req_id = uuid::Uuid::new_v4().to_string();
        Exchange {
            request: CapturedRequest {
                id: req_id.clone(),
                method: method.to_string(),
                url: format!("https://{host}{path}"),
                path: path.to_string(),
                host: host.to_string(),
                headers: HashMap::new(),
                body: None,
                timestamp: chrono::Utc::now(),
                session: "test".to_string(),
            },
            response: Some(CapturedResponse {
                id: uuid::Uuid::new_v4().to_string(),
                request_id: req_id,
                status,
                status_text: "OK".to_string(),
                headers: HashMap::new(),
                body: None,
                timestamp: chrono::Utc::now(),
                latency_ms,
            }),
        }
    }

    #[tokio::test]
    async fn test_compute_stats_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let pool = crate::db::init_db(&db_path).await.unwrap();

        let stats = compute_session_stats(&pool, "test").await.unwrap();
        assert_eq!(stats.total_requests, 0);
        assert_eq!(stats.avg_latency_ms, 0);
    }

    #[tokio::test]
    async fn test_compute_stats_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");
        let pool = crate::db::init_db(&db_path).await.unwrap();

        // Create session first (FK constraint)
        let session = crate::models::Session::new("test".to_string(), db_path.display().to_string());
        crate::db::create_session(&pool, &session).await.unwrap();

        // Store some exchanges
        let exchanges = vec![
            make_exchange("GET", "/users", "api.example.com", 200, 42),
            make_exchange("POST", "/users", "api.example.com", 201, 120),
            make_exchange("GET", "/users", "api.example.com", 200, 35),
            make_exchange("GET", "/profile", "api.example.com", 500, 80),
            make_exchange("DELETE", "/users/1", "api.example.com", 204, 60),
        ];

        for ex in &exchanges {
            crate::db::store_request(&pool, &ex.request).await.unwrap();
            if let Some(ref resp) = ex.response {
                crate::db::store_response(&pool, resp).await.unwrap();
            }
        }

        let stats = compute_session_stats(&pool, "test").await.unwrap();
        assert_eq!(stats.total_requests, 5);
        assert_eq!(stats.avg_latency_ms, (42 + 120 + 35 + 80 + 60) / 5);
        assert!((stats.error_rate_percent - 20.0).abs() < 0.01); // 1 error out of 5

        assert_eq!(*stats.method_breakdown.get("GET").unwrap(), 3);
        assert_eq!(*stats.method_breakdown.get("POST").unwrap(), 1);

        assert_eq!(stats.top_endpoints[0].0, "GET /users");
        assert_eq!(stats.top_endpoints[0].1, 2);
    }

    #[test]
    fn test_format_stats() {
        let stats = SessionStats {
            total_requests: 10,
            avg_latency_ms: 75,
            error_rate_percent: 10.0,
            status_breakdown: {
                let mut h = HashMap::new();
                h.insert(200, 8);
                h.insert(500, 1);
                h.insert(404, 1);
                h
            },
            top_endpoints: vec![("GET /api".to_string(), 5)],
            latency_histogram: vec![
                ("0-50".to_string(), 3),
                ("50-100".to_string(), 5),
                ("100-250".to_string(), 2),
                ("250-500".to_string(), 0),
                ("500-1000".to_string(), 0),
                ("1000-2500".to_string(), 0),
                ("2500+".to_string(), 0),
            ],
            method_breakdown: {
                let mut h = HashMap::new();
                h.insert("GET".to_string(), 8);
                h.insert("POST".to_string(), 2);
                h
            },
            host_breakdown: {
                let mut h = HashMap::new();
                h.insert("api.example.com".to_string(), 10);
                h
            },
        };

        let formatted = format_stats(&stats, "test-session");
        assert!(formatted.contains("Total requests: 10"));
        assert!(formatted.contains("Average latency: 75 ms"));
        assert!(formatted.contains("Error rate: 10.0%"));
        assert!(formatted.contains("GET /api: 5"));
    }
}
