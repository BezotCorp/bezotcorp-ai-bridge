use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{chat_reques::ChatRequest, normalizer::strip_json_fence};

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
    pub fn try_normalize_raw_tool_call(content: &str, request: &ChatRequest) -> Option<Self> {
        let cleaned = strip_json_fence(content.trim())?;
        let raw: RawToolCall = serde_json::from_str(cleaned).ok()?;

        let allowed = request
            .tools
            .iter()
            .filter(|tool| tool.tool_type == "function")
            .map(|tool| tool.function.name.as_str())
            .collect::<HashSet<_>>();

        if !allowed.contains(raw.name.as_str()) {
            return None;
        }

        if !raw.arguments.is_object() {
            return None;
        }

        Some(Self {
            id: "call_ollama_proxy_1".to_string(),
            call_type: "function".to_string(),
            function: ToolCallFunctionChunk {
                name: raw.name,
                arguments: serde_json::to_string(&raw.arguments).ok()?,
            },
        })
    }
}

#[derive(Debug, Serialize)]
pub struct ToolCallFunctionChunk {
    pub name: String,
    pub arguments: String,
}
