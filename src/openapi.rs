//! OpenAPI 3.0 spec generation from captured HTTP traffic.
//!
//! Analyzes captured requests/responses and builds a complete OpenAPI
//! spec with paths, methods, parameters, and inferred schemas.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;

use crate::models::Exchange;

/// An OpenAPI 3.0.3 specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiSpec {
    pub openapi: String,
    pub info: OpenApiInfo,
    pub servers: Vec<OpenApiServer>,
    pub paths: HashMap<String, OpenApiPathItem>,
    pub components: Option<OpenApiComponents>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiInfo {
    pub title: String,
    pub version: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiServer {
    pub url: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiPathItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub get: Option<OpenApiOperation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post: Option<OpenApiOperation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub put: Option<OpenApiOperation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete: Option<OpenApiOperation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<OpenApiOperation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<OpenApiOperation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<OpenApiOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiOperation {
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<OpenApiParameter>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body: Option<OpenApiRequestBody>,
    pub responses: HashMap<String, OpenApiResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiParameter {
    pub name: String,
    #[serde(rename = "in")]
    pub location: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiRequestBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub content: HashMap<String, OpenApiMediaType>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiMediaType {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiResponse {
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<HashMap<String, OpenApiMediaType>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiComponents {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schemas: Option<HashMap<String, serde_json::Value>>,
}

/// Generate an OpenAPI spec from all captured traffic in a session.
pub async fn generate_from_session(pool: &SqlitePool, session: &str) -> Result<OpenApiSpec> {
    let exchanges = crate::db::list_exchanges(pool, session, 10000).await?;
    Ok(generate_from_exchanges(&exchanges, session))
}

/// Generate an OpenAPI spec from a list of exchanges.
pub fn generate_from_exchanges(exchanges: &[Exchange], session: &str) -> OpenApiSpec {
    let mut paths: HashMap<String, OpenApiPathItem> = HashMap::new();
    let mut schemas: HashMap<String, serde_json::Value> = HashMap::new();
    let mut hosts: std::collections::HashSet<String> = std::collections::HashSet::new();

    for exchange in exchanges {
        hosts.insert(exchange.request.host.clone());

        let path = exchange.request.path.clone();
        let method = exchange.request.method.to_lowercase();

        // Extract path parameters from the path itself
        let (normalized_path, path_params) = extract_path_params(&path);

        let operation = build_operation(exchange, &path_params);

        let path_item = paths
            .entry(normalized_path)
            .or_insert_with(|| OpenApiPathItem {
                get: None,
                post: None,
                put: None,
                delete: None,
                patch: None,
                head: None,
                options: None,
            });

        match method.as_str() {
            "get" => path_item.get = Some(operation),
            "post" => path_item.post = Some(operation),
            "put" => path_item.put = Some(operation),
            "delete" => path_item.delete = Some(operation),
            "patch" => path_item.patch = Some(operation),
            "head" => path_item.head = Some(operation),
            "options" => path_item.options = Some(operation),
            _ => {}
        }
    }

    // Try to infer a schema from response bodies
    if let Some(schema) = infer_schema_from_responses(exchanges) {
        schemas.insert("InferredResponse".to_string(), schema);
    }

    let server_url = hosts
        .iter()
        .next()
        .map(|h| format!("https://{}", h))
        .unwrap_or_else(|| "https://api.example.com".to_string());

    OpenApiSpec {
        openapi: "3.0.3".to_string(),
        info: OpenApiInfo {
            title: format!("Wireclaw Generated API — {}", session),
            version: "1.0.0".to_string(),
            description: format!(
                "Auto-generated OpenAPI spec from captured traffic in session '{}'",
                session
            ),
        },
        servers: vec![OpenApiServer {
            url: server_url,
            description: "Inferred from captured traffic".to_string(),
        }],
        paths,
        components: if !schemas.is_empty() {
            Some(OpenApiComponents {
                schemas: Some(schemas),
            })
        } else {
            None
        },
    }
}

fn build_operation(exchange: &Exchange, path_params: &[(String, String)]) -> OpenApiOperation {
    let mut parameters = Vec::new();

    // Add path parameters
    for (name, _example) in path_params {
        parameters.push(OpenApiParameter {
            name: name.clone(),
            location: "path".to_string(),
            required: true,
            schema: Some(json!({ "type": "string" })),
            description: Some(format!("Path parameter: {}", name)),
        });
    }

    // Add query parameters from URL
    if let Some(query) = exchange.request.url.split('?').nth(1) {
        for param in query.split('&') {
            if let Some((name, _value)) = param.split_once('=') {
                parameters.push(OpenApiParameter {
                    name: name.to_string(),
                    location: "query".to_string(),
                    required: false,
                    schema: Some(json!({ "type": "string" })),
                    description: None,
                });
            }
        }
    }

    // Add common headers as parameters
    for header_name in exchange.request.headers.keys() {
        if is_common_header(header_name) {
            parameters.push(OpenApiParameter {
                name: header_name.clone(),
                location: "header".to_string(),
                required: false,
                schema: Some(json!({ "type": "string" })),
                description: None,
            });
        }
    }

    let request_body = if exchange.request.body.is_some() {
        let mut content = HashMap::new();
        content.insert(
            "application/json".to_string(),
            OpenApiMediaType {
                schema: Some(json!({ "type": "object" })),
                example: exchange
                    .request
                    .body
                    .as_ref()
                    .and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok()),
            },
        );
        Some(OpenApiRequestBody {
            description: Some("Request body".to_string()),
            content,
            required: true,
        })
    } else {
        None
    };

    let mut responses = HashMap::new();
    if let Some(ref resp) = exchange.response {
        let status_code = resp.status.to_string();
        let mut content = HashMap::new();

        if let Some(ref body) = resp.body {
            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(body) {
                content.insert(
                    "application/json".to_string(),
                    OpenApiMediaType {
                        schema: Some(json!({ "type": "object" })),
                        example: Some(json),
                    },
                );
            } else {
                content.insert(
                    "text/plain".to_string(),
                    OpenApiMediaType {
                        schema: Some(json!({ "type": "string" })),
                        example: Some(serde_json::Value::String(
                            String::from_utf8_lossy(body).to_string(),
                        )),
                    },
                );
            }
        }

        responses.insert(
            status_code,
            OpenApiResponse {
                description: resp.status_text.clone(),
                content: if content.is_empty() {
                    None
                } else {
                    Some(content)
                },
            },
        );
    } else {
        responses.insert(
            "200".to_string(),
            OpenApiResponse {
                description: "Successful response".to_string(),
                content: None,
            },
        );
    }

    OpenApiOperation {
        summary: format!("{} {}", exchange.request.method, exchange.request.path),
        description: Some(format!("Captured from {}", exchange.request.host)),
        parameters: if parameters.is_empty() {
            None
        } else {
            Some(parameters)
        },
        request_body,
        responses,
    }
}

fn extract_path_params(path: &str) -> (String, Vec<(String, String)>) {
    let mut normalized = String::new();
    let mut params = Vec::new();
    let mut in_param = false;
    let mut current = String::new();

    for ch in path.chars() {
        match ch {
            '{' => {
                if !current.is_empty() {
                    normalized.push_str(&current);
                    current.clear();
                }
                in_param = true;
                normalized.push('{');
            }
            '}' => {
                if in_param {
                    params.push((current.clone(), current.clone()));
                    normalized.push('}');
                    current.clear();
                    in_param = false;
                } else {
                    normalized.push(ch);
                }
            }
            '/' => {
                if !current.is_empty() {
                    if in_param {
                        normalized.push_str(&current);
                    } else {
                        // Check if segment looks like an ID (numeric or UUID-like)
                        if is_id_like(&current) {
                            let param_name = infer_param_name(&current);
                            params.push((param_name.clone(), current.clone()));
                            normalized.push('{');
                            normalized.push_str(&param_name);
                            normalized.push('}');
                        } else {
                            normalized.push_str(&current);
                        }
                    }
                    current.clear();
                }
                normalized.push('/');
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        if in_param {
            normalized.push_str(&current);
        } else if is_id_like(&current) {
            let param_name = infer_param_name(&current);
            params.push((param_name.clone(), current.clone()));
            normalized.push('{');
            normalized.push_str(&param_name);
            normalized.push('}');
        } else {
            normalized.push_str(&current);
        }
    }

    (normalized, params)
}

fn is_id_like(segment: &str) -> bool {
    segment.parse::<i64>().is_ok()
        || segment.len() == 36 && segment.chars().filter(|&c| c == '-').count() == 4
}

fn infer_param_name(segment: &str) -> String {
    if is_id_like(segment) {
        "id".to_string()
    } else {
        "param".to_string()
    }
}

fn is_common_header(name: &str) -> bool {
    let common = ["authorization", "content-type", "accept", "x-request-id"];
    common.contains(&name.to_lowercase().as_str())
}

fn infer_schema_from_responses(exchanges: &[Exchange]) -> Option<serde_json::Value> {
    let mut all_fields: HashMap<String, Vec<String>> = HashMap::new();

    for exchange in exchanges {
        if let Some(ref resp) = exchange.response
            && let Some(ref body) = resp.body
            && let Ok(json) = serde_json::from_slice::<serde_json::Value>(body)
        {
            extract_field_types(&json, "", &mut all_fields);
        }
    }

    if all_fields.is_empty() {
        return None;
    }

    let mut properties = serde_json::Map::new();
    for (field, types) in all_fields {
        let inferred_type = infer_type(&types);
        properties.insert(field, json!({ "type": inferred_type }));
    }

    Some(json!({
        "type": "object",
        "properties": properties
    }))
}

fn extract_field_types(
    value: &serde_json::Value,
    prefix: &str,
    fields: &mut HashMap<String, Vec<String>>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map {
                let field_name = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                match val {
                    serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                        extract_field_types(val, &field_name, fields);
                    }
                    _ => {
                        fields.entry(field_name).or_default().push(json_type(val));
                    }
                }
            }
        }
        serde_json::Value::Array(arr) => {
            if let Some(first) = arr.first() {
                extract_field_types(first, prefix, fields);
            }
        }
        _ => {
            fields
                .entry(prefix.to_string())
                .or_default()
                .push(json_type(value));
        }
    }
}

fn json_type(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(_) => "boolean".to_string(),
        serde_json::Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer".to_string()
            } else {
                "number".to_string()
            }
        }
        serde_json::Value::String(_) => "string".to_string(),
        serde_json::Value::Array(_) => "array".to_string(),
        serde_json::Value::Object(_) => "object".to_string(),
    }
}

fn infer_type(types: &[String]) -> String {
    let unique: std::collections::HashSet<_> = types.iter().cloned().collect();
    if unique.len() == 1 {
        types[0].clone()
    } else {
        "string".to_string() // fallback
    }
}

use serde_json::json;
