use crate::{chat_request::ChatRequest, normalizer::strip_json_fence};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
struct RawToolCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Serialize)]
pub struct ToolCallChunk {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolCallFunctionChunk,
}

impl ToolCallChunk {
    pub fn try_normalize_raw_tool_call(content: &str, request: &ChatRequest) -> Option<Vec<Self>> {
        let cleaned = strip_json_fence(content.trim())?;

        let raw_calls: Vec<RawToolCall> = if cleaned.trim_start().starts_with('[') {
            serde_json::from_str(cleaned).ok()?
        } else {
            let single: RawToolCall = serde_json::from_str(cleaned).ok()?;
            vec![single]
        };

        if raw_calls.is_empty() {
            return None;
        }

        let allowed = request
            .tools
            .iter()
            .filter(|tool| tool.tool_type == "function")
            .map(|tool| tool.function.name.as_str())
            .collect::<HashSet<_>>();

        let mut chunks = Vec::with_capacity(raw_calls.len());

        for (index, raw) in raw_calls.into_iter().enumerate() {
            if !allowed.contains(raw.name.as_str()) {
                return None;
            }
            if !raw.arguments.is_object() {
                return None;
            }

            chunks.push(Self {
                id: format!("call_ollama_proxy_{}", index + 1),
                call_type: "function".to_string(),
                function: ToolCallFunctionChunk {
                    name: raw.name,
                    arguments: serde_json::to_string(&raw.arguments).ok()?,
                },
            });
        }

        Some(chunks)
    }
}

#[derive(Debug, Serialize)]
pub struct ToolCallFunctionChunk {
    pub name: String,
    pub arguments: String,
}
