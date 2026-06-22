use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    #[serde(flatten)]
    pub rest: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionSpec,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FunctionSpec {
    pub name: String,
    #[serde(default)]
    pub parameters: Value,
}

#[derive(Debug, Serialize)]
pub struct ToolCallChunk {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolCallFunctionChunk,
}

#[derive(Debug, Serialize)]
pub struct ToolCallFunctionChunk {
    pub name: String,
    pub arguments: String,
}
