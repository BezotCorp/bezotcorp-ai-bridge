use crate::{chat_reques::ChatRequest, openai::ToolCallChunk};
use serde_json::{Value, json};
use std::time::Duration;

/// Au-delà de cette taille de contenu texte accumulé sans JSON valide détecté,
/// on abandonne la détection de tool-call et on bascule en passthrough.
pub const RAW_TOOL_CALL_PASSTHROUGH_THRESHOLD: usize = 4_096;

/// Si aucun octet n'arrive d'Ollama pendant cette durée, on considère le
/// stream mort et on coupe proprement plutôt que de laisser Kilo attendre.
pub const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(90);

/// Nombre de messages récents à inspecter pour détecter un tool result.
pub const TOOL_RESULT_LOOKBACK: usize = 8;

pub fn collect_sse_content(
    bytes: &[u8],
    content: &mut String,
    first_chunk: &mut Option<Value>,
    usage_chunk: &mut Option<Value>,
) {
    let text = String::from_utf8_lossy(bytes);

    for line in text.lines() {
        let Some(data) = line.strip_prefix("data: ") else {
            continue;
        };

        if data == "[DONE]" {
            continue;
        }

        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };

        if first_chunk.is_none() {
            *first_chunk = Some(value.clone());
        }

        if value.get("usage").is_some() {
            *usage_chunk = Some(value.clone());
        }

        if let Some(delta_content) = value["choices"][0]["delta"]["content"].as_str() {
            content.push_str(delta_content);
        }
    }
}

pub fn request_has_recent_tool_result(request: &ChatRequest) -> bool {
    request
        .rest
        .get("messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .rev()
                .take(TOOL_RESULT_LOOKBACK)
                .any(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
        })
        .unwrap_or(false)
}

pub fn build_tool_call_sse(
    model: &str,
    first_chunk: Option<Value>,
    usage_chunk: Option<Value>,
    tool_call: ToolCallChunk,
) -> String {
    let id = first_chunk
        .as_ref()
        .and_then(|v| v.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl-ollama-proxy");

    let created = first_chunk
        .as_ref()
        .and_then(|v| v.get("created"))
        .and_then(Value::as_i64)
        .unwrap_or(0);

    let first = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "tool_calls": [{
                    "index": 0,
                    "id": tool_call.id,
                    "type": tool_call.call_type,
                    "function": {
                        "name": tool_call.function.name,
                        "arguments": ""
                    }
                }]
            },
            "finish_reason": null
        }]
    });

    let args = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "function": {
                        "arguments": tool_call.function.arguments
                    }
                }]
            },
            "finish_reason": null
        }]
    });

    let finish = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "tool_calls"
        }]
    });

    let mut out = String::new();

    out.push_str("data: ");
    out.push_str(&first.to_string());
    out.push_str("\n\n");

    out.push_str("data: ");
    out.push_str(&args.to_string());
    out.push_str("\n\n");

    out.push_str("data: ");
    out.push_str(&finish.to_string());
    out.push_str("\n\n");

    if let Some(usage) = usage_chunk {
        out.push_str("data: ");
        out.push_str(&usage.to_string());
        out.push_str("\n\n");
    }

    out.push_str("data: [DONE]\n\n");
    out
}
