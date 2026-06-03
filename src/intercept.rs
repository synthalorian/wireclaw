//! Request interception / breakpoint system.
//!
//! When `--intercept` is enabled, matching requests are paused before
//! forwarding, allowing the user to view, modify, forward, or drop them.


use anyhow::{Context, Result};
use regex::Regex;

use crate::models::CapturedRequest;

/// A rule that determines which requests to intercept.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct InterceptRule {
    pub method: Option<Regex>,
    pub path: Option<Regex>,
    pub host: Option<Regex>,
}

impl InterceptRule {
    /// Parse a rule from a filter expression string.
    /// Syntax: `method=POST,path=/api/.*,host=api.example.com`
    #[allow(dead_code)]
    pub fn parse(expr: &str) -> Result<Self> {
        let mut method = None;
        let mut path = None;
        let mut host = None;

        for part in expr.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            let Some((key, value)) = part.split_once('=') else {
                anyhow::bail!("invalid intercept rule: '{part}' (expected key=value)");
            };

            let pattern = Regex::new(value.trim())
                .map_err(|e| anyhow::anyhow!("invalid regex in intercept rule: {e}"))?;

            match key.trim().to_lowercase().as_str() {
                "method" => method = Some(pattern),
                "path" => path = Some(pattern),
                "host" => host = Some(pattern),
                _ => anyhow::bail!("unknown intercept rule key: '{key}'"),
            }
        }

        Ok(Self { method, path, host })
    }

    /// Check if a request matches this rule.
    #[allow(dead_code)]
    pub fn matches(&self, req: &CapturedRequest) -> bool {
        if let Some(ref re) = self.method {
            if !re.is_match(&req.method) {
                return false;
            }
        }
        if let Some(ref re) = self.path {
            if !re.is_match(&req.path) {
                return false;
            }
        }
        if let Some(ref re) = self.host {
            if !re.is_match(&req.host) {
                return false;
            }
        }
        true
    }
}

/// Decision made by the user for an intercepted request.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum InterceptDecision {
    /// Forward the request as-is.
    Forward,
    /// Drop the request (return 502 to client).
    Drop,
    /// Forward with modifications (not yet implemented).
    Modify,
}

/// Intercept engine that holds active rules and handles user interaction.
#[allow(dead_code)]
pub struct InterceptEngine {
    rules: Vec<InterceptRule>,
    interactive: bool,
}

impl InterceptEngine {
    #[allow(dead_code)]
    pub fn new(rules: Vec<InterceptRule>, interactive: bool) -> Self {
        Self { rules, interactive }
    }

    /// Check if a request should be intercepted.
    #[allow(dead_code)]
    pub fn should_intercept(&self, req: &CapturedRequest) -> bool {
        self.rules.iter().any(|rule| rule.matches(req))
    }

    /// Prompt the user for a decision on an intercepted request.
    #[allow(dead_code)]
    pub async fn prompt(&self, req: &CapturedRequest) -> Result<(InterceptDecision, Option<CapturedRequest>)> {
        if !self.interactive {
            // Non-interactive mode: auto-forward all intercepted requests
            return Ok((InterceptDecision::Forward, None));
        }

        eprintln!("\n[ledger] INTERCEPTED {} {}", req.method, req.url);
        eprintln!("  Host: {}", req.host);
        eprintln!("  Path: {}", req.path);
        if !req.headers.is_empty() {
            eprintln!("  Headers:");
            for (k, v) in &req.headers {
                eprintln!("    {k}: {v}");
            }
        }
        if let Some(ref body) = req.body {
            let preview = String::from_utf8_lossy(body);
            let preview = if preview.len() > 200 {
                format!("{}...", &preview[..200])
            } else {
                preview.to_string()
            };
            eprintln!("  Body: {preview}");
        }
        eprintln!();
        eprintln!("  [f]orward  [d]rop  [m]odify  [q]uit intercepting");
        eprint!("  > ");

        let mut input = String::new();
        tokio::io::AsyncBufReadExt::read_line(
            &mut tokio::io::BufReader::new(tokio::io::stdin()),
            &mut input,
        )
        .await?;

        let decision = match input.trim().to_lowercase().as_str() {
            "d" | "drop" => InterceptDecision::Drop,
            "m" | "modify" => {
                // For now, modify opens editor same as replay --edit
                let modified = edit_request_in_editor(req).await?;
                return Ok((InterceptDecision::Modify, Some(modified)));
            }
            "q" | "quit" => {
                // Return forward but signal to stop intercepting
                return Ok((InterceptDecision::Forward, None));
            }
            _ => InterceptDecision::Forward,
        };

        Ok((decision, None))
    }
}

/// Serialize a request to JSON, open it in $EDITOR, parse it back.
#[allow(dead_code)]
async fn edit_request_in_editor(req: &CapturedRequest) -> Result<CapturedRequest> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());

    let json = serde_json::to_string_pretty(req)
        .map_err(|e| anyhow::anyhow!("serializing request to JSON: {e}"))?;

    let mut tmp = tempfile::NamedTempFile::with_suffix(".json")?;
    std::io::Write::write_all(&mut tmp, json.as_bytes())?;
    let path = tmp.path().to_path_buf();
    drop(tmp);

    let status = tokio::process::Command::new(&editor)
        .arg(&path)
        .status()
        .await
        .with_context(|| format!("failed to spawn editor: {editor}"))?;

    if !status.success() {
        anyhow::bail!("editor exited with non-zero status");
    }

    let edited_json = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| anyhow::anyhow!("reading edited request file: {e}"))?;

    let edited_req: CapturedRequest = serde_json::from_str(&edited_json)
        .map_err(|e| anyhow::anyhow!("parsing edited request JSON: {e}"))?;

    Ok(edited_req)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_request(method: &str, path: &str, host: &str) -> CapturedRequest {
        CapturedRequest {
            id: uuid::Uuid::new_v4().to_string(),
            method: method.to_string(),
            url: format!("https://{host}{path}"),
            path: path.to_string(),
            host: host.to_string(),
            headers: HashMap::new(),
            body: None,
            timestamp: chrono::Utc::now(),
            session: "test".to_string(),
        }
    }

    #[test]
    fn test_rule_parse_and_match() {
        let rule = InterceptRule::parse("method=POST,path=/api/.*").unwrap();
        assert!(rule.matches(&make_request("POST", "/api/users", "api.example.com")));
        assert!(!rule.matches(&make_request("GET", "/api/users", "api.example.com")));
        assert!(!rule.matches(&make_request("POST", "/other", "api.example.com")));
    }

    #[test]
    fn test_rule_host_match() {
        let rule = InterceptRule::parse("host=api\\.example\\.com").unwrap();
        assert!(rule.matches(&make_request("GET", "/", "api.example.com")));
        assert!(!rule.matches(&make_request("GET", "/", "other.com")));
    }

    #[test]
    fn test_engine_should_intercept() {
        let rules = vec![
            InterceptRule::parse("method=DELETE").unwrap(),
            InterceptRule::parse("path=/admin/.*").unwrap(),
        ];
        let engine = InterceptEngine::new(rules, true);

        assert!(engine.should_intercept(&make_request("DELETE", "/users/1", "api.example.com")));
        assert!(engine.should_intercept(&make_request("GET", "/admin/users", "api.example.com")));
        assert!(!engine.should_intercept(&make_request("GET", "/users", "api.example.com")));
    }

    #[test]
    fn test_rule_empty_matches_all() {
        let rule = InterceptRule::parse("").unwrap();
        assert!(rule.matches(&make_request("GET", "/", "any.com")));
        assert!(rule.matches(&make_request("POST", "/api", "other.com")));
    }
}
