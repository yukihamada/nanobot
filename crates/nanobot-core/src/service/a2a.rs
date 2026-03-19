//! A2A (Agent-to-Agent) protocol client.
//!
//! Implements the client side of the A2A protocol for calling external agents.
//! Uses JSON-RPC 2.0 over HTTPS to send tasks to remote agent servers.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Agent Card
// ---------------------------------------------------------------------------

/// Agent Card as defined by the A2A protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub skills: Vec<AgentSkill>,
}

/// A skill advertised by an A2A agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkill {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
}

/// Fetch the Agent Card from `{base_url}/.well-known/agent.json`.
pub async fn fetch_agent_card(base_url: &str) -> Result<AgentCard, String> {
    let url = format!("{}/.well-known/agent.json", base_url.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch agent card from {}: {}", url, e))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Agent card request returned {}: {}", status, body));
    }

    resp.json::<AgentCard>()
        .await
        .map_err(|e| format!("Failed to parse agent card: {}", e))
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 task send
// ---------------------------------------------------------------------------

/// JSON-RPC 2.0 request envelope.
#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    method: &'static str,
    id: String,
    params: serde_json::Value,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    #[allow(dead_code)]
    id: Option<serde_json::Value>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[allow(dead_code)]
    data: Option<serde_json::Value>,
}

/// Send a task to an A2A agent via JSON-RPC 2.0 `tasks/send`.
///
/// - `base_url`: The agent's base URL (e.g. `https://stayflow.example.com`)
/// - `api_key`: Bearer token for authentication
/// - `skill_id`: The skill to invoke (e.g. `get_reservations`)
/// - `input`: The input payload for the task
pub async fn send_task(
    base_url: &str,
    api_key: &str,
    skill_id: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let url = format!("{}/a2a", base_url.trim_end_matches('/'));
    let request_id = format!("a2a-{}", uuid_v4_simple());

    let rpc_request = JsonRpcRequest {
        jsonrpc: "2.0",
        method: "tasks/send",
        id: request_id.clone(),
        params: serde_json::json!({
            "id": request_id,
            "message": {
                "role": "user",
                "parts": [
                    {
                        "type": "function_call",
                        "skill_id": skill_id,
                        "input": input,
                    }
                ]
            }
        }),
    };

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&rpc_request)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| {
            error!("A2A request to {} failed: {}", url, e);
            format!("A2A request failed: {}", e)
        })?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        warn!("A2A auth failed for {}: 401 Unauthorized", url);
        return Err("A2A authentication failed (401)".to_string());
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        error!("A2A request to {} returned {}: {}", url, status, body);
        return Err(format!("A2A request returned {}: {}", status, body));
    }

    let rpc_resp: JsonRpcResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse A2A JSON-RPC response: {}", e))?;

    if let Some(err) = rpc_resp.error {
        error!("A2A JSON-RPC error: code={}, message={}", err.code, err.message);
        return Err(format!("A2A error ({}): {}", err.code, err.message));
    }

    match rpc_resp.result {
        Some(result) => {
            info!("A2A task {} completed successfully", skill_id);
            Ok(result)
        }
        None => Err("A2A response contained neither result nor error".to_string()),
    }
}

// ---------------------------------------------------------------------------
// A2A tool definitions for StayFlow
// ---------------------------------------------------------------------------

/// Tool name prefix for StayFlow A2A tools.
pub const STAYFLOW_TOOL_PREFIX: &str = "stayflow_";

/// StayFlow A2A tool names (admin-only, conditional on env vars).
pub const STAYFLOW_TOOL_NAMES: &[&str] = &[
    "stayflow_reservations",
    "stayflow_occupancy",
    "stayflow_revenue",
    "stayflow_create_account",
    "stayflow_add_property",
    "stayflow_add_reservation",
    "stayflow_properties",
];

/// Build StayFlow A2A tool definitions (OpenAI function-calling format).
pub fn stayflow_tool_definitions() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "stayflow_reservations",
                "description": "StayFlowから予約一覧を取得。民泊・旅館の予約データ。/ Fetch reservations from StayFlow PMS.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "date_from": {
                            "type": "string",
                            "description": "開始日 (YYYY-MM-DD)"
                        },
                        "date_to": {
                            "type": "string",
                            "description": "終了日 (YYYY-MM-DD)"
                        },
                        "property_id": {
                            "type": "string",
                            "description": "物件ID (省略で全物件)"
                        }
                    }
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "stayflow_occupancy",
                "description": "StayFlowから稼働率を取得。月単位の稼働率。/ Get occupancy rates from StayFlow.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "month": {
                            "type": "string",
                            "description": "対象月 (YYYY-MM)"
                        },
                        "property_id": {
                            "type": "string",
                            "description": "物件ID (省略で全物件)"
                        }
                    }
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "stayflow_revenue",
                "description": "StayFlowから売上サマリを取得。/ Get revenue summary from StayFlow.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "period": {
                            "type": "string",
                            "description": "期間 (today, this_week, this_month, last_month)"
                        },
                        "property_id": {
                            "type": "string",
                            "description": "物件ID (省略で全物件)"
                        }
                    }
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "stayflow_create_account",
                "description": "StayFlowのアカウントを新規作成。民泊管理を始めたいユーザーに使う。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "ユーザー名"
                        },
                        "email": {
                            "type": "string",
                            "description": "メールアドレス"
                        }
                    },
                    "required": ["name", "email"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "stayflow_add_property",
                "description": "StayFlowに新しい物件を登録。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "user_id": {
                            "type": "string",
                            "description": "StayFlowユーザーID"
                        },
                        "name": {
                            "type": "string",
                            "description": "物件名"
                        },
                        "address": {
                            "type": "string",
                            "description": "住所"
                        },
                        "property_type": {
                            "type": "string",
                            "description": "物件タイプ (apartment, house, hotel, ryokan)"
                        },
                        "max_guests": {
                            "type": "integer",
                            "description": "最大宿泊人数"
                        },
                        "base_price": {
                            "type": "number",
                            "description": "1泊の基本料金(JPY)"
                        }
                    },
                    "required": ["user_id", "name"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "stayflow_add_reservation",
                "description": "StayFlowに新しい予約を登録。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "user_id": {
                            "type": "string",
                            "description": "StayFlowユーザーID"
                        },
                        "property_id": {
                            "type": "string",
                            "description": "物件ID"
                        },
                        "guest_name": {
                            "type": "string",
                            "description": "宿泊者名"
                        },
                        "check_in": {
                            "type": "string",
                            "description": "チェックイン日 (YYYY-MM-DD)"
                        },
                        "check_out": {
                            "type": "string",
                            "description": "チェックアウト日 (YYYY-MM-DD)"
                        },
                        "num_guests": {
                            "type": "integer",
                            "description": "宿泊人数"
                        },
                        "total_price": {
                            "type": "number",
                            "description": "合計金額(JPY)"
                        }
                    },
                    "required": ["user_id", "property_id", "guest_name", "check_in", "check_out"]
                }
            }
        }),
        serde_json::json!({
            "type": "function",
            "function": {
                "name": "stayflow_properties",
                "description": "StayFlowの物件一覧を取得。",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "user_id": {
                            "type": "string",
                            "description": "StayFlowユーザーID"
                        }
                    },
                    "required": ["user_id"]
                }
            }
        }),
    ]
}

/// Execute a StayFlow A2A tool call.
/// Maps tool names to A2A skill IDs and sends the task.
pub async fn execute_stayflow_tool(
    tool_name: &str,
    args: &HashMap<String, serde_json::Value>,
) -> String {
    let base_url = match std::env::var("STAYFLOW_A2A_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => return "[TOOL_ERROR] STAYFLOW_A2A_URL not configured".to_string(),
    };
    let api_key = match std::env::var("STAYFLOW_A2A_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => return "[TOOL_ERROR] STAYFLOW_A2A_KEY not configured".to_string(),
    };

    // Map tool name to A2A skill ID: stayflow_reservations → get_reservations
    let skill_id = match tool_name {
        "stayflow_reservations" => "get_reservations",
        "stayflow_occupancy" => "get_occupancy",
        "stayflow_revenue" => "get_revenue_summary",
        "stayflow_create_account" => "create_account",
        "stayflow_add_property" => "add_property",
        "stayflow_add_reservation" => "add_reservation",
        "stayflow_properties" => "get_properties",
        _ => return format!("[TOOL_ERROR] Unknown StayFlow tool: {}", tool_name),
    };

    let input = serde_json::json!(args);

    match send_task(&base_url, &api_key, skill_id, input).await {
        Ok(result) => {
            // Format result for LLM consumption
            match serde_json::to_string_pretty(&result) {
                Ok(pretty) => pretty,
                Err(_) => result.to_string(),
            }
        }
        Err(e) => format!("[TOOL_ERROR] StayFlow A2A: {}", e),
    }
}

/// Check if StayFlow A2A integration is configured (env vars present).
pub fn is_stayflow_configured() -> bool {
    std::env::var("STAYFLOW_A2A_URL").map(|v| !v.is_empty()).unwrap_or(false)
        && std::env::var("STAYFLOW_A2A_KEY").map(|v| !v.is_empty()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Simple UUID v4-ish generator (no external crate needed).
fn uuid_v4_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = now.subsec_nanos();
    let secs = now.as_secs();
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        secs as u32,
        (nanos >> 16) & 0xffff,
        nanos & 0xfff,
        0x8000 | (nanos & 0x3fff),
        secs.wrapping_mul(nanos as u64) & 0xffffffffffff,
    )
}
