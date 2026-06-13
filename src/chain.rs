//! Request chaining with variable extraction.
//!
//! Replay a sequence of requests, extracting values from responses
//! via JSONPath-like syntax and substituting them into subsequent requests.

use std::collections::HashMap;

use anyhow::Result;
use serde_json::Value;

use crate::db;
use crate::models::CapturedRequest;
use crate::replay::ReplayEngine;
use sqlx::SqlitePool;

/// A step in a replay chain.
#[derive(Debug, Clone)]
pub struct ChainStep {
    pub request_id: String,
    pub extracts: Vec<Extract>,
}

/// Extract a value from a response and store it in a variable.
#[derive(Debug, Clone)]
pub struct Extract {
    pub var_name: String,
    pub json_path: Vec<String>,
}

impl Extract {
    /// Parse a JSONPath-like expression: `$.data.token` or `data.token`
    pub fn parse_path(expr: &str) -> Vec<String> {
        let trimmed = expr.trim();
        let without_root = trimmed.strip_prefix("$").unwrap_or(trimmed);
        let without_dot = without_root.strip_prefix('.').unwrap_or(without_root);
        without_dot.split('.').map(|s| s.to_string()).collect()
    }
}

/// Extract a value from JSON using a path of keys.
pub fn extract_json_value(json: &Value, path: &[String]) -> Option<Value> {
    let mut current = json;
    for key in path {
        match current {
            Value::Object(map) => current = map.get(key)?,
            _ => return None,
        }
    }
    Some(current.clone())
}

/// Substitute variables into a request (URL, headers, body).
pub fn substitute_vars(req: &mut CapturedRequest, vars: &HashMap<String, String>) {
    // Substitute in URL
    for (name, value) in vars {
        let placeholder = format!("{{{name}}}");
        req.url = req.url.replace(&placeholder, value);
        req.path = req.path.replace(&placeholder, value);
    }

    // Substitute in headers
    for (_k, v) in req.headers.iter_mut() {
        for (name, value) in vars {
            let placeholder = format!("{{{name}}}");
            *v = v.replace(&placeholder, value);
        }
    }

    // Substitute in body
    if let Some(ref mut body) = req.body {
        let body_str = String::from_utf8_lossy(body).to_string();
        let mut new_body = body_str.clone();
        for (name, value) in vars {
            let placeholder = format!("{{{name}}}");
            new_body = new_body.replace(&placeholder, value);
        }
        if new_body != body_str {
            *body = new_body.into_bytes();
        }
    }
}

/// Chain replay engine.
pub struct ChainEngine {
    pool: SqlitePool,
}

impl ChainEngine {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Replay a chain of requests, extracting variables along the way.
    pub async fn replay_chain(
        &self,
        steps: &[ChainStep],
        _dry_run: bool,
    ) -> Result<HashMap<String, String>> {
        let mut vars: HashMap<String, String> = HashMap::new();
        let replay = ReplayEngine::new(self.pool.clone());

        for (i, step) in steps.iter().enumerate() {
            eprintln!(
                "[ledger] chain step {}/{}: request {}",
                i + 1,
                steps.len(),
                step.request_id
            );

            // Fetch the request
            let mut exchange = db::get_request_by_id(&self.pool, &step.request_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("request not found: {}", step.request_id))?;

            // Substitute variables
            substitute_vars(&mut exchange.request, &vars);

            // Replay
            let response = replay.replay_exchange_for_chain(&exchange).await?;

            // Extract variables from response
            for extract in &step.extracts {
                if let Some(ref body) = response.body {
                    if let Ok(json) = serde_json::from_slice::<Value>(body) {
                        if let Some(value) = extract_json_value(&json, &extract.json_path) {
                            let value_str = match value {
                                Value::String(s) => s,
                                other => other.to_string(),
                            };
                            eprintln!("[ledger] extracted ${{{}}} = {value_str}", extract.var_name);
                            vars.insert(extract.var_name.clone(), value_str);
                        } else {
                            eprintln!(
                                "[ledger] warning: JSON path not found for ${{{}}}",
                                extract.var_name
                            );
                        }
                    } else {
                        eprintln!(
                            "[ledger] warning: response body is not valid JSON, cannot extract ${{{}}}",
                            extract.var_name
                        );
                    }
                } else {
                    eprintln!(
                        "[ledger] warning: no response body to extract ${{{}}}",
                        extract.var_name
                    );
                }
            }
        }

        Ok(vars)
    }

    /// Parse a chain definition from a simple string format.
    /// Format: `req_id1:extract1,extract2;req_id2:extract3`
    /// Extract format: `var_name=json.path`
    pub fn parse_chain(expr: &str) -> Result<Vec<ChainStep>> {
        let mut steps = Vec::new();

        for step_str in expr.split(';') {
            let step_str = step_str.trim();
            if step_str.is_empty() {
                continue;
            }

            let Some((req_id, extracts_str)) = step_str.split_once(':') else {
                anyhow::bail!("invalid chain step: '{step_str}' (expected request_id:extracts)");
            };

            let mut extracts = Vec::new();
            for extract_str in extracts_str.split(',') {
                let extract_str = extract_str.trim();
                if extract_str.is_empty() {
                    continue;
                }

                let Some((var_name, path_expr)) = extract_str.split_once('=') else {
                    anyhow::bail!("invalid extract: '{extract_str}' (expected var_name=json.path)");
                };

                extracts.push(Extract {
                    var_name: var_name.trim().to_string(),
                    json_path: Extract::parse_path(path_expr.trim()),
                });
            }

            steps.push(ChainStep {
                request_id: req_id.trim().to_string(),
                extracts,
            });
        }

        Ok(steps)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_path() {
        assert_eq!(Extract::parse_path("$.data.token"), vec!["data", "token"]);
        assert_eq!(Extract::parse_path("data.token"), vec!["data", "token"]);
        assert_eq!(Extract::parse_path("id"), vec!["id"]);
    }

    #[test]
    fn test_extract_json_value() {
        let json = serde_json::json!({
            "data": {
                "token": "abc123",
                "user": { "id": 42 }
            }
        });

        assert_eq!(
            extract_json_value(&json, &["data".to_string(), "token".to_string()]),
            Some(Value::String("abc123".to_string()))
        );
        assert_eq!(
            extract_json_value(
                &json,
                &["data".to_string(), "user".to_string(), "id".to_string()]
            ),
            Some(Value::Number(42.into()))
        );
        assert_eq!(
            extract_json_value(&json, &["nonexistent".to_string()]),
            None
        );
    }

    #[test]
    fn test_substitute_vars() {
        let mut req = CapturedRequest {
            id: "test".to_string(),
            method: "GET".to_string(),
            url: "https://api.example.com/users/{user_id}".to_string(),
            path: "/users/{user_id}".to_string(),
            host: "api.example.com".to_string(),
            headers: {
                let mut h = HashMap::new();
                h.insert("authorization".to_string(), "Bearer {token}".to_string());
                h
            },
            body: Some(br#"{"user_id":"{user_id}"}"#.to_vec()),
            timestamp: chrono::Utc::now(),
            session: "test".to_string(),
        };

        let mut vars = HashMap::new();
        vars.insert("user_id".to_string(), "42".to_string());
        vars.insert("token".to_string(), "secret123".to_string());

        substitute_vars(&mut req, &vars);

        assert_eq!(req.url, "https://api.example.com/users/42");
        assert_eq!(req.path, "/users/42");
        assert_eq!(
            req.headers.get("authorization").unwrap(),
            "Bearer secret123"
        );
        assert_eq!(req.body, Some(br#"{"user_id":"42"}"#.to_vec()));
    }

    #[test]
    fn test_parse_chain() {
        let steps =
            ChainEngine::parse_chain("req1:token=data.token;req2:user_id=data.user.id").unwrap();

        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].request_id, "req1");
        assert_eq!(steps[0].extracts[0].var_name, "token");
        assert_eq!(steps[0].extracts[0].json_path, vec!["data", "token"]);

        assert_eq!(steps[1].request_id, "req2");
        assert_eq!(steps[1].extracts[0].var_name, "user_id");
        assert_eq!(steps[1].extracts[0].json_path, vec!["data", "user", "id"]);
    }

    #[test]
    fn test_parse_chain_no_extracts() {
        let steps = ChainEngine::parse_chain("req1:;req2:").unwrap();
        assert_eq!(steps.len(), 2);
        assert!(steps[0].extracts.is_empty());
        assert!(steps[1].extracts.is_empty());
    }
}
