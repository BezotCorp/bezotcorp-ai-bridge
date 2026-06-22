use crate::openai::{ChatRequest, ToolCallChunk, ToolCallFunctionChunk};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Deserialize)]
struct RawToolCall {
    name: String,
    arguments: Value,
}

pub fn try_normalize_raw_tool_call(content: &str, request: &ChatRequest) -> Option<ToolCallChunk> {
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

    Some(ToolCallChunk {
        id: "call_ollama_proxy_1".to_string(),
        call_type: "function".to_string(),
        function: ToolCallFunctionChunk {
            name: raw.name,
            arguments: serde_json::to_string(&raw.arguments).ok()?,
        },
    })
}

pub(crate) fn strip_json_fence(input: &str) -> Option<&str> {
    if input.starts_with("```json") && input.ends_with("```") {
        return Some(
            input
                .trim_start_matches("```json")
                .trim_end_matches("```")
                .trim(),
        );
    }

    if input.starts_with("```") && input.ends_with("```") {
        return Some(
            input
                .trim_start_matches("```")
                .trim_end_matches("```")
                .trim(),
        );
    }

    if input.starts_with('{') && input.ends_with('}') {
        return Some(input);
    }

    None
}
