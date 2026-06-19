//! Request comparison and diff utilities.
//!
//! Compare two captured requests/responses and show structural differences.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use crate::models::Exchange;

/// Result of comparing two exchanges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResult {
    pub request_id_a: String,
    pub request_id_b: String,
    pub differences: Vec<DiffItem>,
    pub summary: DiffSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffItem {
    pub field: String,
    pub kind: DiffKind,
    pub left: Option<String>,
    pub right: Option<String>,
    pub nested: Option<Vec<DiffItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffKind {
    Added,
    Removed,
    Changed,
    Same,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffSummary {
    pub total_differences: usize,
    pub header_differences: usize,
    pub body_differences: usize,
    pub status_different: bool,
    pub latency_different: bool,
}

/// Compare two exchanges and return a detailed diff.
pub fn compare_exchanges(a: &Exchange, b: &Exchange) -> DiffResult {
    let mut differences = Vec::new();
    let mut header_diffs = 0;
    let mut body_diffs = 0;
    let mut status_diff = false;
    let mut latency_diff = false;

    // Compare request methods
    if a.request.method != b.request.method {
        differences.push(DiffItem {
            field: "request.method".to_string(),
            kind: DiffKind::Changed,
            left: Some(a.request.method.clone()),
            right: Some(b.request.method.clone()),
            nested: None,
        });
    }

    // Compare request paths
    if a.request.path != b.request.path {
        differences.push(DiffItem {
            field: "request.path".to_string(),
            kind: DiffKind::Changed,
            left: Some(a.request.path.clone()),
            right: Some(b.request.path.clone()),
            nested: None,
        });
    }

    // Compare request hosts
    if a.request.host != b.request.host {
        differences.push(DiffItem {
            field: "request.host".to_string(),
            kind: DiffKind::Changed,
            left: Some(a.request.host.clone()),
            right: Some(b.request.host.clone()),
            nested: None,
        });
    }

    // Compare request headers
    let header_diff = compare_headers(&a.request.headers, &b.request.headers);
    if !header_diff.is_empty() {
        header_diffs = header_diff.len();
        differences.push(DiffItem {
            field: "request.headers".to_string(),
            kind: DiffKind::Changed,
            left: None,
            right: None,
            nested: Some(header_diff),
        });
    }

    // Compare request bodies
    let body_diff = compare_bodies(a.request.body.as_deref(), b.request.body.as_deref());
    if !body_diff.is_empty() {
        body_diffs = body_diff.len();
        differences.push(DiffItem {
            field: "request.body".to_string(),
            kind: DiffKind::Changed,
            left: None,
            right: None,
            nested: Some(body_diff),
        });
    }

    // Compare responses
    match (&a.response, &b.response) {
        (Some(ra), Some(rb)) => {
            if ra.status != rb.status {
                status_diff = true;
                differences.push(DiffItem {
                    field: "response.status".to_string(),
                    kind: DiffKind::Changed,
                    left: Some(ra.status.to_string()),
                    right: Some(rb.status.to_string()),
                    nested: None,
                });
            }

            if ra.status_text != rb.status_text {
                differences.push(DiffItem {
                    field: "response.status_text".to_string(),
                    kind: DiffKind::Changed,
                    left: Some(ra.status_text.clone()),
                    right: Some(rb.status_text.clone()),
                    nested: None,
                });
            }

            if ra.latency_ms != rb.latency_ms {
                latency_diff = true;
                differences.push(DiffItem {
                    field: "response.latency_ms".to_string(),
                    kind: DiffKind::Changed,
                    left: Some(ra.latency_ms.to_string()),
                    right: Some(rb.latency_ms.to_string()),
                    nested: None,
                });
            }

            let resp_header_diff = compare_headers(&ra.headers, &rb.headers);
            if !resp_header_diff.is_empty() {
                header_diffs += resp_header_diff.len();
                differences.push(DiffItem {
                    field: "response.headers".to_string(),
                    kind: DiffKind::Changed,
                    left: None,
                    right: None,
                    nested: Some(resp_header_diff),
                });
            }

            let resp_body_diff = compare_bodies(ra.body.as_deref(), rb.body.as_deref());
            if !resp_body_diff.is_empty() {
                body_diffs += resp_body_diff.len();
                differences.push(DiffItem {
                    field: "response.body".to_string(),
                    kind: DiffKind::Changed,
                    left: None,
                    right: None,
                    nested: Some(resp_body_diff),
                });
            }
        }
        (Some(_), None) => {
            differences.push(DiffItem {
                field: "response".to_string(),
                kind: DiffKind::Removed,
                left: Some("present".to_string()),
                right: None,
                nested: None,
            });
        }
        (None, Some(_)) => {
            differences.push(DiffItem {
                field: "response".to_string(),
                kind: DiffKind::Added,
                left: None,
                right: Some("present".to_string()),
                nested: None,
            });
        }
        (None, None) => {}
    }

    DiffResult {
        request_id_a: a.request.id.clone(),
        request_id_b: b.request.id.clone(),
        differences,
        summary: DiffSummary {
            total_differences: header_diffs
                + body_diffs
                + if status_diff { 1 } else { 0 }
                + if latency_diff { 1 } else { 0 },
            header_differences: header_diffs,
            body_differences: body_diffs,
            status_different: status_diff,
            latency_different: latency_diff,
        },
    }
}

fn compare_headers(a: &HashMap<String, String>, b: &HashMap<String, String>) -> Vec<DiffItem> {
    let mut diffs = Vec::new();
    let mut all_keys: std::collections::HashSet<&String> = a.keys().collect();
    all_keys.extend(b.keys());

    for key in all_keys {
        let a_val = a.get(key);
        let b_val = b.get(key);

        match (a_val, b_val) {
            (Some(av), Some(bv)) => {
                if av != bv {
                    diffs.push(DiffItem {
                        field: key.clone(),
                        kind: DiffKind::Changed,
                        left: Some(av.clone()),
                        right: Some(bv.clone()),
                        nested: None,
                    });
                }
            }
            (Some(av), None) => {
                diffs.push(DiffItem {
                    field: key.clone(),
                    kind: DiffKind::Removed,
                    left: Some(av.clone()),
                    right: None,
                    nested: None,
                });
            }
            (None, Some(bv)) => {
                diffs.push(DiffItem {
                    field: key.clone(),
                    kind: DiffKind::Added,
                    left: None,
                    right: Some(bv.clone()),
                    nested: None,
                });
            }
            (None, None) => {}
        }
    }

    diffs
}

fn compare_bodies(a: Option<&[u8]>, b: Option<&[u8]>) -> Vec<DiffItem> {
    let mut diffs = Vec::new();

    match (a, b) {
        (Some(a_body), Some(b_body)) => {
            // Try to compare as JSON
            if let (Ok(a_json), Ok(b_json)) = (
                serde_json::from_slice::<Value>(a_body),
                serde_json::from_slice::<Value>(b_body),
            ) {
                diffs.extend(compare_json_values(&a_json, &b_json, ""));
            } else {
                // Compare as strings
                let a_str = String::from_utf8_lossy(a_body);
                let b_str = String::from_utf8_lossy(b_body);
                if a_str != b_str {
                    diffs.push(DiffItem {
                        field: "body".to_string(),
                        kind: DiffKind::Changed,
                        left: Some(a_str.to_string()),
                        right: Some(b_str.to_string()),
                        nested: None,
                    });
                }
            }
        }
        (Some(_), None) => {
            diffs.push(DiffItem {
                field: "body".to_string(),
                kind: DiffKind::Removed,
                left: Some("present".to_string()),
                right: None,
                nested: None,
            });
        }
        (None, Some(_)) => {
            diffs.push(DiffItem {
                field: "body".to_string(),
                kind: DiffKind::Added,
                left: None,
                right: Some("present".to_string()),
                nested: None,
            });
        }
        (None, None) => {}
    }

    diffs
}

fn compare_json_values(a: &Value, b: &Value, path: &str) -> Vec<DiffItem> {
    let mut diffs = Vec::new();

    match (a, b) {
        (Value::Object(a_map), Value::Object(b_map)) => {
            let mut all_keys: std::collections::HashSet<&String> = a_map.keys().collect();
            all_keys.extend(b_map.keys());

            for key in all_keys {
                let new_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };

                match (a_map.get(key), b_map.get(key)) {
                    (Some(av), Some(bv)) => {
                        diffs.extend(compare_json_values(av, bv, &new_path));
                    }
                    (Some(_), None) => {
                        diffs.push(DiffItem {
                            field: new_path,
                            kind: DiffKind::Removed,
                            left: Some(format_json(a_map.get(key).unwrap())),
                            right: None,
                            nested: None,
                        });
                    }
                    (None, Some(_)) => {
                        diffs.push(DiffItem {
                            field: new_path,
                            kind: DiffKind::Added,
                            left: None,
                            right: Some(format_json(b_map.get(key).unwrap())),
                            nested: None,
                        });
                    }
                    (None, None) => {}
                }
            }
        }
        (Value::Array(a_arr), Value::Array(b_arr)) => {
            let max_len = a_arr.len().max(b_arr.len());
            for i in 0..max_len {
                let new_path = format!("{}[{}]", path, i);
                match (a_arr.get(i), b_arr.get(i)) {
                    (Some(av), Some(bv)) => {
                        diffs.extend(compare_json_values(av, bv, &new_path));
                    }
                    (Some(_), None) => {
                        diffs.push(DiffItem {
                            field: new_path,
                            kind: DiffKind::Removed,
                            left: Some(format_json(a_arr.get(i).unwrap())),
                            right: None,
                            nested: None,
                        });
                    }
                    (None, Some(_)) => {
                        diffs.push(DiffItem {
                            field: new_path,
                            kind: DiffKind::Added,
                            left: None,
                            right: Some(format_json(b_arr.get(i).unwrap())),
                            nested: None,
                        });
                    }
                    (None, None) => {}
                }
            }
        }
        _ => {
            if a != b {
                diffs.push(DiffItem {
                    field: path.to_string(),
                    kind: DiffKind::Changed,
                    left: Some(format_json(a)),
                    right: Some(format_json(b)),
                    nested: None,
                });
            }
        }
    }

    diffs
}

fn format_json(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    }
}

/// Format a diff result for terminal display.
pub fn format_diff_terminal(result: &DiffResult) -> String {
    let mut output = String::new();
    output.push_str("\n🦞 Wireclaw Diff\n");
    output.push_str(&format!(
        "  {} vs {}\n\n",
        result.request_id_a, result.request_id_b
    ));

    if result.summary.total_differences == 0 {
        output.push_str("  ✅ No differences found!\n");
        return output;
    }

    output.push_str(&format!(
        "  Summary: {} differences ({} headers, {} body)\n\n",
        result.summary.total_differences,
        result.summary.header_differences,
        result.summary.body_differences
    ));

    for item in &result.differences {
        output.push_str(&format_diff_item(item, 0));
    }

    output
}

fn format_diff_item(item: &DiffItem, depth: usize) -> String {
    let indent = "  ".repeat(depth + 1);
    let mut output = String::new();

    let emoji = match item.kind {
        DiffKind::Added => "➕",
        DiffKind::Removed => "➖",
        DiffKind::Changed => "🔀",
        DiffKind::Same => "✅",
    };

    match (&item.left, &item.right) {
        (Some(left), Some(right)) => {
            output.push_str(&format!(
                "{}{} {}: {} → {}\n",
                indent, emoji, item.field, left, right
            ));
        }
        (Some(left), None) => {
            output.push_str(&format!("{}{} {}: {}\n", indent, emoji, item.field, left));
        }
        (None, Some(right)) => {
            output.push_str(&format!("{}{} {}: {}\n", indent, emoji, item.field, right));
        }
        (None, None) => {
            if let Some(ref nested) = item.nested {
                output.push_str(&format!("{}{} {}:\n", indent, emoji, item.field));
                for child in nested {
                    output.push_str(&format_diff_item(child, depth + 1));
                }
            }
        }
    }

    output
}
