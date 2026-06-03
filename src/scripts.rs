//! Pre/post request Lua scripting engine.
//!
//! Allows users to write Lua hooks that run before a request is sent
//! (pre-request) or after a response is received (post-response).
//!
//! Pre-request scripts can:
//! - Modify request headers
//! - Change the request body
//! - Set variables for later use
//!
//! Post-response scripts can:
//! - Assert on status codes
//! - Extract data from JSON responses
//! - Log custom metrics
//!
//! Example pre-request.lua:
//! ```lua
//! -- Add an auth header
//! request:set_header("Authorization", "Bearer " .. vars.token)
//! -- Modify the body
//! request:set_body('{"modified": true}')
//! ```
//!
//! Example post-response.lua:
//! ```lua
//! -- Assert status is 200
//! assert(response.status == 200, "expected 200, got " .. response.status)
//! -- Extract token from JSON
//! vars.token = response:json().data.token
//! ```

use std::cell::RefCell;
use std::collections::HashMap;

use anyhow::{Context, Result};
use mlua::{Lua, Value};

use crate::models::{CapturedRequest, CapturedResponse};

/// Script engine that manages Lua state and executes hooks.
pub struct ScriptEngine {
    lua: Lua,
    vars: RefCell<HashMap<String, String>>,
}

impl ScriptEngine {
    pub fn new() -> Result<Self> {
        let lua = Lua::new();
        Ok(Self {
            lua,
            vars: RefCell::new(HashMap::new()),
        })
    }

    /// Load variables into the Lua globals.
    pub fn load_vars(&self, vars: HashMap<String, String>) {
        *self.vars.borrow_mut() = vars;
    }

    /// Run a pre-request script, returning modifications to apply.
    pub fn run_pre_request(&self, script: &str, request: &CapturedRequest) -> Result<RequestMods> {
        let globals = self.lua.globals();

        // Set up the request table (read-only view)
        let req_table = self.lua.create_table()?;
        req_table.set("method", request.method.clone())?;
        req_table.set("url", request.url.clone())?;
        req_table.set("path", request.path.clone())?;
        req_table.set("host", request.host.clone())?;

        let headers = self.lua.create_table()?;
        for (k, v) in &request.headers {
            headers.set(k.clone(), v.clone())?;
        }
        req_table.set("headers", headers)?;

        let body_str = request
            .body
            .as_ref()
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or("")
            .to_string();
        req_table.set("body", body_str)?;

        globals.set("request", req_table)?;

        // Set up vars table (read-write via userdata)
        let vars_table = self.lua.create_table()?;
        for (k, v) in self.vars.borrow().iter() {
            vars_table.set(k.clone(), v.clone())?;
        }
        globals.set("vars", vars_table.clone())?;

        // Create a mutable modifications table that the script can write to
        let mods = self.lua.create_table()?;
        globals.set("__mods", mods.clone())?;

        // Inject helper functions that write to __mods
        let set_header = self
            .lua
            .create_function(|lua, (key, value): (String, String)| {
                let mods: mlua::Table = lua.globals().get("__mods")?;
                let headers: mlua::Table = match mods.get("headers")? {
                    Some(h) => h,
                    None => {
                        let h = lua.create_table()?;
                        mods.set("headers", h.clone())?;
                        h
                    }
                };
                headers.set(key, value)?;
                Ok(())
            })?;
        globals.set("set_header", set_header)?;

        let set_body = self.lua.create_function(|lua, body: String| {
            let mods: mlua::Table = lua.globals().get("__mods")?;
            mods.set("body", body)?;
            Ok(())
        })?;
        globals.set("set_body", set_body)?;

        let set_url = self.lua.create_function(|lua, url: String| {
            let mods: mlua::Table = lua.globals().get("__mods")?;
            mods.set("url", url)?;
            Ok(())
        })?;
        globals.set("set_url", set_url)?;

        // Also expose request:set_header etc. for nicer API
        let req_set_header = self
            .lua
            .create_function(|lua, (req, key, value): (mlua::Table, String, String)| {
                let _ = req;
                let mods: mlua::Table = lua.globals().get("__mods")?;
                let headers: mlua::Table = match mods.get("headers")? {
                    Some(h) => h,
                    None => {
                        let h = lua.create_table()?;
                        mods.set("headers", h.clone())?;
                        h
                    }
                };
                headers.set(key, value)?;
                Ok(())
            })?;
        let req_table: mlua::Table = globals.get("request")?;
        req_table.set("set_header", req_set_header)?;

        let req_set_body = self
            .lua
            .create_function(|lua, (req, body): (mlua::Table, String)| {
                let _ = req;
                let mods: mlua::Table = lua.globals().get("__mods")?;
                mods.set("body", body)?;
                Ok(())
            })?;
        req_table.set("set_body", req_set_body)?;

        let req_set_url = self
            .lua
            .create_function(|lua, (req, url): (mlua::Table, String)| {
                let _ = req;
                let mods: mlua::Table = lua.globals().get("__mods")?;
                mods.set("url", url)?;
                Ok(())
            })?;
        req_table.set("set_url", req_set_url)?;

        // Execute script
        self.lua
            .load(script)
            .exec()
            .context("pre-request script error")?;

        // Collect modifications
        let mut result = RequestMods::default();

        if let Ok(url) = mods.get::<String>("url") {
            result.url = Some(url);
        }
        if let Ok(body) = mods.get::<String>("body") {
            result.body = Some(body.into_bytes());
        }
        if let Ok(headers) = mods.get::<mlua::Table>("headers") {
            for pair in headers.pairs::<String, String>() {
                if let Ok((k, v)) = pair {
                    result.headers.insert(k, v);
                }
            }
        }

        // Collect updated vars
        for pair in vars_table.pairs::<String, String>() {
            if let Ok((k, v)) = pair {
                self.vars.borrow_mut().insert(k, v);
            }
        }

        Ok(result)
    }

    /// Run a post-response script.
    pub fn run_post_response(&self, script: &str, response: &CapturedResponse) -> Result<()> {
        let globals = self.lua.globals();

        // Set up the response table
        let resp_table = self.lua.create_table()?;
        resp_table.set("status", response.status)?;
        resp_table.set("status_text", response.status_text.clone())?;

        let headers = self.lua.create_table()?;
        for (k, v) in &response.headers {
            headers.set(k.clone(), v.clone())?;
        }
        resp_table.set("headers", headers)?;

        let body_str = response
            .body
            .as_ref()
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or("")
            .to_string();
        resp_table.set("body", body_str.clone())?;

        // JSON parsing helper
        let body_json = body_str.clone();
        let json_fn = self.lua.create_function(move |lua, ()| {
            match serde_json::from_str::<serde_json::Value>(&body_json) {
                Ok(val) => json_value_to_lua(lua, &val),
                Err(e) => Err(mlua::Error::RuntimeError(format!("JSON parse error: {e}"))),
            }
        })?;
        resp_table.set("json", json_fn)?;

        globals.set("response", resp_table)?;

        // Set up vars table
        let vars_table = self.lua.create_table()?;
        for (k, v) in self.vars.borrow().iter() {
            vars_table.set(k.clone(), v.clone())?;
        }
        globals.set("vars", vars_table.clone())?;

        // Execute script
        self.lua
            .load(script)
            .exec()
            .context("post-response script error")?;

        // Collect updated vars
        for pair in vars_table.pairs::<String, String>() {
            if let Ok((k, v)) = pair {
                self.vars.borrow_mut().insert(k, v);
            }
        }

        Ok(())
    }

    /// Get the current variables.
    pub fn get_vars(&self) -> HashMap<String, String> {
        self.vars.borrow().clone()
    }
}

/// Modifications to apply to a request after running a pre-request script.
#[derive(Debug, Default, Clone)]
pub struct RequestMods {
    pub url: Option<String>,
    pub body: Option<Vec<u8>>,
    pub headers: HashMap<String, String>,
}

impl RequestMods {
    /// Apply modifications to a request.
    pub fn apply(&self, request: &mut CapturedRequest) {
        if let Some(ref url) = self.url {
            request.url = url.clone();
            request.path = url.clone();
        }
        if let Some(ref body) = self.body {
            request.body = Some(body.clone());
        }
        for (k, v) in &self.headers {
            request.headers.insert(k.clone(), v.clone());
        }
    }
}

/// Convert a serde_json::Value to a mlua::Value.
fn json_value_to_lua(lua: &Lua, value: &serde_json::Value) -> Result<Value, mlua::Error> {
    match value {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(b) => Ok(Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Number(f))
            } else {
                Ok(Value::Nil)
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(lua.create_string(s)?)),
        serde_json::Value::Array(arr) => {
            let table = lua.create_table()?;
            for (i, v) in arr.iter().enumerate() {
                table.set(i + 1, json_value_to_lua(lua, v)?)?;
            }
            Ok(Value::Table(table))
        }
        serde_json::Value::Object(obj) => {
            let table = lua.create_table()?;
            for (k, v) in obj.iter() {
                table.set(k.clone(), json_value_to_lua(lua, v)?)?;
            }
            Ok(Value::Table(table))
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_request() -> CapturedRequest {
        CapturedRequest {
            id: "test".to_string(),
            method: "GET".to_string(),
            url: "https://api.example.com/users".to_string(),
            path: "/users".to_string(),
            host: "api.example.com".to_string(),
            headers: {
                let mut h = HashMap::new();
                h.insert("content-type".to_string(), "application/json".to_string());
                h
            },
            body: None,
            timestamp: chrono::Utc::now(),
            session: "test".to_string(),
        }
    }

    fn make_response() -> CapturedResponse {
        CapturedResponse {
            id: "resp".to_string(),
            request_id: "test".to_string(),
            status: 200,
            status_text: "OK".to_string(),
            headers: {
                let mut h = HashMap::new();
                h.insert("content-type".to_string(), "application/json".to_string());
                h
            },
            body: Some(br#"{"data":{"token":"abc123"}}"#.to_vec()),
            timestamp: chrono::Utc::now(),
            latency_ms: 42,
        }
    }

    #[test]
    fn test_pre_request_set_header() {
        let engine = ScriptEngine::new().unwrap();
        let mut req = make_request();

        let mods = engine
            .run_pre_request(
                "request:set_header('Authorization', 'Bearer secret')",
                &req,
            )
            .unwrap();

        mods.apply(&mut req);
        assert_eq!(req.headers.get("Authorization").unwrap(), "Bearer secret");
    }

    #[test]
    fn test_pre_request_set_body() {
        let engine = ScriptEngine::new().unwrap();
        let mut req = make_request();

        let mods = engine
            .run_pre_request("request:set_body('{\"modified\":true}')", &req)
            .unwrap();

        mods.apply(&mut req);
        assert_eq!(req.body, Some(br#"{"modified":true}"#.to_vec()));
    }

    #[test]
    fn test_pre_request_vars() {
        let engine = ScriptEngine::new().unwrap();
        let mut vars = HashMap::new();
        vars.insert("token".to_string(), "mytoken".to_string());
        engine.load_vars(vars);

        let mut req = make_request();
        let mods = engine
            .run_pre_request(
                "request:set_header('Authorization', 'Bearer ' .. vars.token)",
                &req,
            )
            .unwrap();

        mods.apply(&mut req);
        assert_eq!(
            req.headers.get("Authorization").unwrap(),
            "Bearer mytoken"
        );
        assert_eq!(engine.get_vars().get("token").unwrap(), "mytoken");
    }

    #[test]
    fn test_post_response_json_extraction() {
        let engine = ScriptEngine::new().unwrap();
        engine.load_vars(HashMap::new());

        let resp = make_response();
        engine
            .run_post_response("vars.token = response:json().data.token", &resp)
            .unwrap();

        assert_eq!(engine.get_vars().get("token").unwrap(), "abc123");
    }

    #[test]
    fn test_post_response_assert_status() {
        let engine = ScriptEngine::new().unwrap();
        let resp = make_response();

        let result = engine.run_post_response(
            "assert(response.status == 200, 'bad status')",
            &resp,
        );
        assert!(result.is_ok());

        let bad_resp = CapturedResponse {
            status: 500,
            ..make_response()
        };
        let result =
            engine.run_post_response("assert(response.status == 200, 'bad status')", &bad_resp);
        assert!(result.is_err());
    }

    #[test]
    fn test_post_response_body_access() {
        let engine = ScriptEngine::new().unwrap();
        engine.load_vars(HashMap::new());

        let resp = make_response();
        engine
            .run_post_response("vars.body_len = tostring(#response.body)", &resp)
            .unwrap();

        assert_eq!(engine.get_vars().get("body_len").unwrap(), "27");
    }
}
