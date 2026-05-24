use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::Path;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::engine::{run_bre, BreRunOptions};
use crate::config::{BreConfig, serialize_prompt_payload};

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct RunBreArgs {
    path: String,
    max_tokens: Option<usize>,
    skip_diagnostics: Option<bool>,
    report: Option<bool>,
    format: Option<String>,
    #[serde(default)]
    js_aliases: Option<HashMap<String, Vec<String>>>,
}

fn handle_run_bre(args_val: Value) -> Result<Value, String> {
    let args: RunBreArgs = serde_json::from_value(args_val)
        .map_err(|err| format!("Invalid arguments: {err}"))?;

    let repo_root = Path::new(&args.path);
    let config_path = repo_root.join("bre.json");
    if !config_path.exists() {
        return Err(format!("bre.json not found in {}", args.path));
    }

    let config_contents = std::fs::read_to_string(&config_path)
        .map_err(|err| format!("Failed to read bre.json: {err}"))?;

    let config: BreConfig = serde_json::from_str(&config_contents)
        .map_err(|err| format!("Failed to parse bre.json: {err}"))?;

    let run_options = BreRunOptions {
        max_tokens: args.max_tokens,
        skip_diagnostics: args.skip_diagnostics.unwrap_or(false),
        include_report: args.report.unwrap_or(false),
        js_aliases: args.js_aliases.unwrap_or_default(),
        scoring_policy: None,
    };

    let payload = run_bre(repo_root, config, run_options)
        .map_err(|err| format!("BRE execution error: {err}"))?;

    let format = args.format.unwrap_or_else(|| "prompt".to_string());
    let formatted_output = match format.as_str() {
        "json" => serde_json::to_string_pretty(&payload)
            .map_err(|err| format!("Failed to serialize payload to JSON: {err}"))?,
        _ => serialize_prompt_payload(&payload),
    };

    Ok(json!({
        "content": [
            {
                "type": "text",
                "text": formatted_output
            }
        ],
        "isError": false
    }))
}

fn handle_message(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let req: JsonRpcRequest = match serde_json::from_str::<JsonRpcRequest>(trimmed) {
        Ok(r) => {
            if r.jsonrpc != "2.0" {
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: r.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32600,
                        message: "Invalid Request: jsonrpc version must be '2.0'".to_string(),
                        data: None,
                    }),
                };
                return Some(serde_json::to_string(&resp).unwrap());
            }
            r
        }
        Err(err) => {
            let resp = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: None,
                result: None,
                error: Some(JsonRpcError {
                    code: -32700,
                    message: format!("Parse error: {err}"),
                    data: None,
                }),
            };
            return Some(serde_json::to_string(&resp).unwrap());
        }
    };

    let response = match req.method.as_str() {
        "initialize" => {
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": env!("CARGO_PKG_NAME"),
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
                error: None,
            }
        }
        "notifications/initialized" => {
            return None;
        }
        "tools/list" => {
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: Some(json!({
                    "tools": [
                        {
                            "name": "run_bre",
                            "description": "Executes the Bounded Reasoning Environment (BRE) context optimization on a repository path using its bre.json configuration, returning the selected context prompt or a structured JSON payload.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": {
                                        "type": "string",
                                        "description": "Absolute path to the repository directory (must contain bre.json)."
                                    },
                                    "max_tokens": {
                                        "type": "integer",
                                        "description": "Optional maximum token limit override."
                                    },
                                    "skip_diagnostics": {
                                        "type": "boolean",
                                        "description": "Optional flag to skip running diagnostics (defaults to false)."
                                    },
                                    "report": {
                                        "type": "boolean",
                                        "description": "Optional flag to request an evidence report summary."
                                    },
                                    "format": {
                                        "type": "string",
                                        "enum": ["prompt", "json"],
                                        "description": "Optional output format: 'prompt' (default) or 'json'."
                                    },
                                    "js_aliases": {
                                        "type": "object",
                                        "additionalProperties": {
                                            "type": "array",
                                            "items": {
                                                "type": "string"
                                            }
                                        },
                                        "description": "Optional JSON map for JS/TS import alias mappings."
                                    }
                                },
                                "required": ["path"]
                            }
                        }
                    ]
                })),
                error: None,
            }
        }
        "tools/call" => {
            let params = req.params.unwrap_or(Value::Null);
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

            if name == "run_bre" {
                let result = match handle_run_bre(arguments) {
                    Ok(val) => val,
                    Err(err_msg) => {
                        json!({
                            "content": [
                                {
                                    "type": "text",
                                    "text": format!("Error: {err_msg}")
                                }
                            ],
                            "isError": true
                        })
                    }
                };

                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: Some(result),
                    error: None,
                }
            } else {
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: req.id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32601,
                        message: format!("Tool not found: {name}"),
                        data: None,
                    }),
                }
            }
        }
        _ => {
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id: req.id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: format!("Method not found: {}", req.method),
                    data: None,
                }),
            }
        }
    };

    Some(serde_json::to_string(&response).unwrap())
}

pub fn run_mcp_server() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = io::BufReader::new(stdin.lock());
    let mut writer = stdout.lock();

    let mut line = String::new();
    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            break;
        }
        if let Some(resp) = handle_message(&line) {
            writer.write_all(resp.as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_initialize() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "test-client",
                    "version": "1.0"
                }
            }
        });
        let line = req.to_string();
        let resp_str = handle_message(&line).unwrap();
        let resp: Value = serde_json::from_str(&resp_str).unwrap();

        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 42);
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(resp["result"]["serverInfo"]["name"], env!("CARGO_PKG_NAME"));
    }

    #[test]
    fn test_mcp_tools_list() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": "list-id",
            "method": "tools/list"
        });
        let line = req.to_string();
        let resp_str = handle_message(&line).unwrap();
        let resp: Value = serde_json::from_str(&resp_str).unwrap();

        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], "list-id");
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "run_bre");
    }

    #[test]
    fn test_mcp_unregistered_method() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "some/weird/method"
        });
        let line = req.to_string();
        let resp_str = handle_message(&line).unwrap();
        let resp: Value = serde_json::from_str(&resp_str).unwrap();

        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 99);
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn test_mcp_initialized_notification_produces_no_response() {
        let req = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let line = req.to_string();
        let resp = handle_message(&line);
        assert!(resp.is_none());
    }

    #[test]
    fn test_mcp_invalid_json_rpc() {
        let line = "{invalid-json";
        let resp_str = handle_message(&line).unwrap();
        let resp: Value = serde_json::from_str(&resp_str).unwrap();

        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], Value::Null);
        assert_eq!(resp["error"]["code"], -32700);
    }

    #[test]
    fn test_mcp_invalid_jsonrpc_version() {
        let req = json!({
            "jsonrpc": "1.0",
            "id": 123,
            "method": "tools/list"
        });
        let line = req.to_string();
        let resp_str = handle_message(&line).unwrap();
        let resp: Value = serde_json::from_str(&resp_str).unwrap();

        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 123);
        assert_eq!(resp["error"]["code"], -32600);
        assert!(resp["error"]["message"].as_str().unwrap().contains("jsonrpc version must be '2.0'"));
    }

    #[test]
    fn test_mcp_run_bre_tool_error_for_missing_config() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "run_bre",
                "arguments": {
                    "path": "/nonexistent/path/at/all"
                }
            }
        });
        let line = req.to_string();
        let resp_str = handle_message(&line).unwrap();
        let resp: Value = serde_json::from_str(&resp_str).unwrap();

        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["isError"], true);
        let error_text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(error_text.contains("bre.json not found"));
    }

    #[test]
    fn test_mcp_run_bre_tool_success() {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

        // Write a valid bre.json
        let config = json!({
            "goal": "Test goal",
            "acceptance_criteria": ["criteria"],
            "assumptions": ["assume"],
            "non_goals": ["non-goal"],
            "active_files": ["src/main.rs"]
        });
        std::fs::write(root.join("bre.json"), config.to_string()).unwrap();

        let req = json!({
            "jsonrpc": "2.0",
            "id": "run-id",
            "method": "tools/call",
            "params": {
                "name": "run_bre",
                "arguments": {
                    "path": root.to_string_lossy().to_string(),
                    "skip_diagnostics": true,
                    "format": "json"
                }
            }
        });

        let line = req.to_string();
        let resp_str = handle_message(&line).unwrap();
        let resp: Value = serde_json::from_str(&resp_str).unwrap();

        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], "run-id");
        assert_eq!(resp["result"]["isError"], false);

        let content_text = resp["result"]["content"][0]["text"].as_str().unwrap();
        let payload: Value = serde_json::from_str(content_text).unwrap();
        assert_eq!(payload["goal"], "Test goal");
        assert_eq!(payload["context"]["active"][0]["path"], "src/main.rs");
    }
}
