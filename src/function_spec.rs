use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub struct FunctionSpec {
    pub name: String,
    #[serde(default)]
    pub parameters: Value,
}
