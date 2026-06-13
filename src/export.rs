//! Export captured traffic to HAR, curl, or raw HTTP formats.

use std::collections::HashMap;

use anyhow::Result;
use serde_json::json;
use sqlx::SqlitePool;
use std::path::Path;

use crate::cli::ExportFormat;
use crate::db;
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
            ExportFormat::Har => self.to_har(&exchanges, session)?,
            ExportFormat::Curl => self.to_curl(&exchanges)?,
            ExportFormat::Raw => self.to_raw(&exchanges)?,
            ExportFormat::Postman => self.to_postman(&exchanges, session)?,
        };

        if let Some(path) = output {
            std::fs::write(path, &content)?;
            eprintln!(
                "[ledger] exported {} exchanges to {}",
                exchanges.len(),
                path.display()
            );
        }

        Ok(content)
    }

    async fn fetch_session_exchanges(&self, session: &str) -> Result<Vec<Exchange>> {
        db::list_exchanges(&self.pool, session, 10000).await
    }

    fn to_har(&self, exchanges: &[Exchange], session: &str) -> Result<String> {
        let entries: Vec<serde_json::Value> = exchanges
            .iter()
            .map(|ex| {
                let req = &ex.request;
                let resp = ex.response.as_ref();

                let req_headers: Vec<serde_json::Value> = req
                    .headers
                    .iter()
                    .map(|(k, v)| json!({ "name": k, "value": v }))
                    .collect();

                let resp_headers: Vec<serde_json::Value> = resp.map_or(Vec::new(), |r| {
                    r.headers
                        .iter()
                        .map(|(k, v)| json!({ "name": k, "value": v }))
                        .collect()
                });

                let req_body_size = req.body.as_ref().map_or(0, |b| b.len() as i64);
                let resp_body_size = resp
                    .and_then(|r| r.body.as_ref().map(|b| b.len() as i64))
                    .unwrap_or(0);

                let req_content = if let Some(ref body) = req.body {
                    json!({
                        "size": req_body_size,
                        "mimeType": guess_mime_type(&req.headers),
                        "text": String::from_utf8_lossy(body).to_string()
                    })
                } else {
                    json!({ "size": 0 })
                };

                let resp_content = if let Some(body) = resp.and_then(|r| r.body.as_ref()) {
                    json!({
                        "size": resp_body_size,
                        "mimeType": resp.map_or("", |r| guess_mime_type(&r.headers)),
                        "text": String::from_utf8_lossy(body).to_string()
                    })
                } else {
                    json!({ "size": 0 })
                };

                json!({
                    "startedDateTime": req.timestamp.to_rfc3339(),
                    "time": resp.map_or(0, |r| r.latency_ms),
                    "request": {
                        "method": req.method,
                        "url": req.url,
                        "httpVersion": "HTTP/1.1",
                        "headers": req_headers,
                        "queryString": [],
                        "cookies": [],
                        "headersSize": -1,
                        "bodySize": req_body_size,
                        "postData": req_content
                    },
                    "response": {
                        "status": resp.map_or(0, |r| r.status),
                        "statusText": resp.map_or("", |r| &r.status_text),
                        "httpVersion": "HTTP/1.1",
                        "headers": resp_headers,
                        "cookies": [],
                        "content": resp_content,
                        "redirectURL": "",
                        "headersSize": -1,
                        "bodySize": resp_body_size
                    },
                    "cache": {},
                    "timings": {
                        "send": 0,
                        "wait": resp.map_or(0, |r| r.latency_ms),
                        "receive": 0
                    }
                })
            })
            .collect();

        let har = json!({
            "log": {
                "version": "1.2",
                "creator": {
                    "name": "ledger",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "pages": [{
                    "startedDateTime": chrono::Utc::now().to_rfc3339(),
                    "id": session,
                    "title": format!("ledger session: {}", session),
                    "pageTimings": { "onContentLoad": -1, "onLoad": -1 }
                }],
                "entries": entries
            }
        });

        Ok(serde_json::to_string_pretty(&har)?)
    }

    fn to_curl(&self, exchanges: &[Exchange]) -> Result<String> {
        let mut lines = Vec::new();
        for ex in exchanges {
            let req = &ex.request;
            let mut cmd = format!("curl -X {} '{}'", req.method, req.url);

            for (k, v) in &req.headers {
                let lower = k.to_lowercase();
                if lower == "host" || lower == "content-length" {
                    continue;
                }
                cmd.push_str(&format!(" -H '{}: {}'", k, v.replace('"', "\\\"")));
            }

            if let Some(ref body) = req.body {
                let body_str = String::from_utf8_lossy(body);
                cmd.push_str(&format!(" -d '{}'", body_str.replace('"', "\\\"")));
            }

            lines.push(cmd);
        }
        Ok(lines.join("\n"))
    }

    fn to_raw(&self, exchanges: &[Exchange]) -> Result<String> {
        let mut output = String::new();
        for (i, ex) in exchanges.iter().enumerate() {
            let req = &ex.request;
            output.push_str(&format!("=== Exchange #{} ===\n", i + 1));
            output.push_str(&format!("{} {} HTTP/1.1\n", req.method, req.url));
            for (k, v) in &req.headers {
                output.push_str(&format!("{}: {}\n", k, v));
            }
            output.push('\n');
            if let Some(ref body) = req.body {
                output.push_str(&String::from_utf8_lossy(body));
                output.push('\n');
            }

            if let Some(ref resp) = ex.response {
                output.push_str(&format!(
                    "\nHTTP/1.1 {} {}\n",
                    resp.status, resp.status_text
                ));
                for (k, v) in &resp.headers {
                    output.push_str(&format!("{}: {}\n", k, v));
                }
                output.push('\n');
                if let Some(ref body) = resp.body {
                    output.push_str(&String::from_utf8_lossy(body));
                    output.push('\n');
                }
            }
            output.push_str("\n---\n\n");
        }
        Ok(output)
    }

    fn to_postman(&self, exchanges: &[Exchange], session: &str) -> Result<String> {
        use std::collections::BTreeMap;

        // Group by host
        let mut groups: BTreeMap<&str, Vec<&Exchange>> = BTreeMap::new();
        for ex in exchanges {
            groups.entry(&ex.request.host).or_default().push(ex);
        }

        let items: Vec<serde_json::Value> = groups
            .iter()
            .map(|(host, group_exchanges)| {
                let group_items: Vec<serde_json::Value> = group_exchanges.iter().map(|ex| {
                let req = &ex.request;

                let header_list: Vec<serde_json::Value> = req.headers.iter()
                    .map(|(k, v)| json!({ "key": k, "value": v, "type": "default" }))
                    .collect();

                let body = if let Some(ref b) = req.body {
                    let mime = guess_mime_type(&req.headers);
                    json!({
                        "mode": "raw",
                        "raw": String::from_utf8_lossy(b).to_string(),
                        "options": {
                            "raw": {
                                "language": if mime.contains("json") { "json" } else { "text" }
                            }
                        }
                    })
                } else {
                    json!({ "mode": "raw", "raw": "" })
                };

                json!({
                    "name": format!("{} {}", req.method, req.path),
                    "request": {
                        "method": req.method,
                        "header": header_list,
                        "url": to_postman_url(&req.url),
                        "body": body
                    },
                    "response": []
                })
            }).collect();

                json!({
                    "name": host,
                    "item": group_items
                })
            })
            .collect();

        let collection = json!({
            "info": {
                "_postman_id": uuid::Uuid::new_v4().to_string(),
                "name": format!("ledger session: {}", session),
                "description": format!("Exported from Ledger session '{}' with {} requests", session, exchanges.len()),
                "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
            },
            "item": items
        });

        Ok(serde_json::to_string_pretty(&collection)?)
    }
}

fn guess_mime_type(headers: &HashMap<String, String>) -> &str {
    headers
        .get("content-type")
        .map(|s| s.split(';').next().unwrap_or(s).trim())
        .unwrap_or("application/octet-stream")
}

fn to_postman_url(url: &str) -> serde_json::Value {
    // Try to parse into host + path + query components
    if let Ok(uri) = url.parse::<hyper::Uri>() {
        let scheme = uri.scheme_str().unwrap_or("https");
        let host = uri.host().unwrap_or("");
        let port = uri.port_u16();
        let path = uri.path();
        let query = uri.query();

        let raw = if let Some(q) = query {
            format!("{}?{}", url, q)
        } else {
            url.to_string()
        };

        let mut url_parts = json!({
            "raw": raw,
            "protocol": scheme,
            "host": host.split('.').collect::<Vec<&str>>(),
            "path": path.trim_start_matches('/').split('/').filter(|s| !s.is_empty()).collect::<Vec<&str>>(),
        });

        if let Some(p) = port {
            url_parts["port"] = json!(p);
        }

        if let Some(q) = query {
            let queries: Vec<serde_json::Value> = q
                .split('&')
                .filter_map(|pair| {
                    let mut parts = pair.splitn(2, '=');
                    let key = parts.next()?;
                    let val = parts.next().unwrap_or("");
                    Some(json!({
                        "key": key,
                        "value": val
                    }))
                })
                .collect();
            if !queries.is_empty() {
                url_parts["query"] = json!(queries);
            }
        }

        url_parts
    } else {
        json!({ "raw": url })
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CapturedRequest, CapturedResponse, Exchange};

    fn make_exchange(method: &str, url: &str, host: &str, path: &str, status: u16) -> Exchange {
        Exchange {
            request: CapturedRequest {
                id: "test-req".to_string(),
                method: method.to_string(),
                url: url.to_string(),
                host: host.to_string(),
                path: path.to_string(),
                headers: std::collections::HashMap::new(),
                body: None,
                timestamp: chrono::Utc::now(),
                session: "test".to_string(),
            },
            response: Some(CapturedResponse {
                id: "test-resp".to_string(),
                request_id: "test-req".to_string(),
                status,
                status_text: "OK".to_string(),
                headers: std::collections::HashMap::new(),
                body: None,
                timestamp: chrono::Utc::now(),
                latency_ms: 42,
            }),
        }
    }

    #[test]
    fn test_postman_url_parsing() {
        let url = to_postman_url("https://api.example.com/v1/users?id=123");
        assert_eq!(url["protocol"], "https");
        assert_eq!(url["host"], json!(["api", "example", "com"]));
        assert_eq!(url["path"], json!(["v1", "users"]));
        assert!(url.get("query").is_some());
    }

    #[test]
    fn test_postman_url_no_query() {
        let url = to_postman_url("https://example.com/api");
        assert_eq!(url["protocol"], "https");
        assert_eq!(url["host"], json!(["example", "com"]));
        assert_eq!(url["path"], json!(["api"]));
        assert!(url.get("query").is_none());
    }

    #[tokio::test]
    async fn test_postman_export_structure() {
        let exchanges = vec![
            make_exchange(
                "GET",
                "https://api.example.com/v1/users",
                "api.example.com",
                "/v1/users",
                200,
            ),
            make_exchange(
                "POST",
                "https://api.example.com/v1/users",
                "api.example.com",
                "/v1/users",
                201,
            ),
            make_exchange("GET", "https://other.com/data", "other.com", "/data", 200),
        ];

        let exporter = Exporter::new_fake().await;
        let json_str = exporter.to_postman(&exchanges, "test-session").unwrap();
        let collection: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(collection["info"]["name"], "ledger session: test-session");
        assert!(
            !collection["info"]["_postman_id"]
                .as_str()
                .unwrap()
                .is_empty()
        );

        let items = collection["item"].as_array().unwrap();
        assert_eq!(items.len(), 2); // 2 groups: api.example.com, other.com

        let api_group = items
            .iter()
            .find(|i| i["name"] == "api.example.com")
            .unwrap();
        assert_eq!(api_group["item"].as_array().unwrap().len(), 2);

        let other_group = items.iter().find(|i| i["name"] == "other.com").unwrap();
        assert_eq!(other_group["item"].as_array().unwrap().len(), 1);
    }

    impl Exporter {
        async fn new_fake() -> Self {
            use sqlx::sqlite::SqlitePoolOptions;
            let pool = SqlitePoolOptions::new()
                .connect("sqlite::memory:")
                .await
                .unwrap();
            Self { pool }
        }
    }
}
