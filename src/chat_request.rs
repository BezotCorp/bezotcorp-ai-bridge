use serde::Deserialize;
use serde_json::Value;

use crate::tool_spec::ToolSpec;

#[derive(Debug, Clone, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    #[serde(flatten)]
    pub rest: Value,
}
