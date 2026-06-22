use serde::Deserialize;

use crate::function_spec::FunctionSpec;

#[derive(Debug, Clone, Deserialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionSpec,
}
