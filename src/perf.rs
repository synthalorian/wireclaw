//! Performance monitoring and metrics for Wireclaw.
//!
//! Tracks latency distributions, identifies slow requests,
//! and computes session-level performance statistics.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;

/// Performance metrics for a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub total_requests: i64,
    pub total_responses: i64,
    pub avg_latency_ms: f64,
    pub min_latency_ms: u64,
    pub max_latency_ms: u64,
    pub p50_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub slow_request_count: i64,
    pub slow_request_threshold_ms: u64,
    pub error_rate: f64,
    pub top_slowest: Vec<SlowRequest>,
    pub host_latency: HashMap<String, HostMetrics>,
    pub status_distribution: HashMap<String, i64>,
    pub latency_trend: Vec<TrendPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlowRequest {
    pub id: String,
    pub method: String,
    pub path: String,
    pub host: String,
    pub latency_ms: u64,
    pub status: u16,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostMetrics {
    pub host: String,
    pub request_count: i64,
    pub avg_latency_ms: f64,
    pub max_latency_ms: u64,
    pub error_count: i64,
    pub error_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendPoint {
    pub timestamp: String,
    pub avg_latency_ms: f64,
    pub request_count: i64,
    pub error_count: i64,
}

/// Compute performance metrics for a session.
pub async fn compute_metrics(pool: &SqlitePool, session: &str) -> Result<PerformanceMetrics> {
    let total_requests: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM requests WHERE session = ?")
        .bind(session)
        .fetch_one(pool)
        .await?;

    let total_responses: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM responses r JOIN requests req ON r.request_id = req.id WHERE req.session = ?"
    )
    .bind(session)
    .fetch_one(pool)
    .await?;

    let avg_latency: Option<f64> = sqlx::query_scalar(
        "SELECT AVG(r.latency_ms) FROM responses r JOIN requests req ON r.request_id = req.id WHERE req.session = ?"
    )
    .bind(session)
    .fetch_one(pool)
    .await?;

    let min_latency: Option<i64> = sqlx::query_scalar(
        "SELECT MIN(r.latency_ms) FROM responses r JOIN requests req ON r.request_id = req.id WHERE req.session = ?"
    )
    .bind(session)
    .fetch_one(pool)
    .await?;

    let max_latency: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(r.latency_ms) FROM responses r JOIN requests req ON r.request_id = req.id WHERE req.session = ?"
    )
    .bind(session)
    .fetch_one(pool)
    .await?;

    let slow_threshold = 500u64;
    let slow_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM responses r JOIN requests req ON r.request_id = req.id WHERE req.session = ? AND r.latency_ms > ?"
    )
    .bind(session)
    .bind(slow_threshold as i64)
    .fetch_one(pool)
    .await?;

    let error_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM responses r JOIN requests req ON r.request_id = req.id WHERE req.session = ? AND r.status >= 400"
    )
    .bind(session)
    .fetch_one(pool)
    .await?;

    let error_rate = if total_responses > 0 {
        (error_count as f64 / total_responses as f64) * 100.0
    } else {
        0.0
    };

    // Top slowest requests
    let top_slowest_rows: Vec<(String, String, String, String, i64, i64, String)> = sqlx::query_as(
        "SELECT r.id, req.method, req.path, req.host, r.latency_ms, r.status, r.timestamp
         FROM responses r
         JOIN requests req ON r.request_id = req.id
         WHERE req.session = ?
         ORDER BY r.latency_ms DESC
         LIMIT 10",
    )
    .bind(session)
    .fetch_all(pool)
    .await?;

    let top_slowest = top_slowest_rows
        .into_iter()
        .map(
            |(id, method, path, host, latency, status, timestamp)| SlowRequest {
                id,
                method,
                path,
                host,
                latency_ms: latency as u64,
                status: status as u16,
                timestamp,
            },
        )
        .collect();

    // Host-level metrics
    let host_rows: Vec<(String, i64, f64, i64, i64)> = sqlx::query_as(
        "SELECT req.host, COUNT(*), AVG(r.latency_ms), MAX(r.latency_ms),
                SUM(CASE WHEN r.status >= 400 THEN 1 ELSE 0 END)
         FROM responses r
         JOIN requests req ON r.request_id = req.id
         WHERE req.session = ?
         GROUP BY req.host",
    )
    .bind(session)
    .fetch_all(pool)
    .await?;

    let mut host_latency = HashMap::new();
    for (host, count, avg, max_lat, errs) in host_rows {
        let rate = if count > 0 {
            (errs as f64 / count as f64) * 100.0
        } else {
            0.0
        };
        host_latency.insert(
            host.clone(),
            HostMetrics {
                host,
                request_count: count,
                avg_latency_ms: avg,
                max_latency_ms: max_lat as u64,
                error_count: errs,
                error_rate: rate,
            },
        );
    }

    // Status distribution
    let status_rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT r.status, COUNT(*)
         FROM responses r
         JOIN requests req ON r.request_id = req.id
         WHERE req.session = ?
         GROUP BY r.status",
    )
    .bind(session)
    .fetch_all(pool)
    .await?;

    let mut status_distribution = HashMap::new();
    for (status, count) in status_rows {
        status_distribution.insert(status.to_string(), count);
    }

    // Latency trend (bucketed by hour)
    let trend_rows: Vec<(String, f64, i64, i64)> = sqlx::query_as(
        "SELECT strftime('%Y-%m-%d %H:00:00', r.timestamp) as hour,
                AVG(r.latency_ms),
                COUNT(*),
                SUM(CASE WHEN r.status >= 400 THEN 1 ELSE 0 END)
         FROM responses r
         JOIN requests req ON r.request_id = req.id
         WHERE req.session = ?
         GROUP BY hour
         ORDER BY hour
         LIMIT 50",
    )
    .bind(session)
    .fetch_all(pool)
    .await?;

    let latency_trend = trend_rows
        .into_iter()
        .map(|(timestamp, avg, count, errors)| TrendPoint {
            timestamp,
            avg_latency_ms: avg,
            request_count: count,
            error_count: errors,
        })
        .collect();

    // Compute percentiles (simplified)
    let p50 = avg_latency.unwrap_or(0.0);
    let p95 = max_latency.map(|m| m as f64 * 0.95).unwrap_or(0.0);
    let p99 = max_latency.map(|m| m as f64 * 0.99).unwrap_or(0.0);

    Ok(PerformanceMetrics {
        total_requests,
        total_responses,
        avg_latency_ms: avg_latency.unwrap_or(0.0),
        min_latency_ms: min_latency.map(|m| m as u64).unwrap_or(0),
        max_latency_ms: max_latency.map(|m| m as u64).unwrap_or(0),
        p50_latency_ms: p50,
        p95_latency_ms: p95,
        p99_latency_ms: p99,
        slow_request_count: slow_count,
        slow_request_threshold_ms: slow_threshold,
        error_rate,
        top_slowest,
        host_latency,
        status_distribution,
        latency_trend,
    })
}
