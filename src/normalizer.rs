pub(crate) const TOOL_CALL_CONTAINER: &str = "```";
pub(crate) const TOOL_CALL_BEGIN_JSON: &str = "```json";
pub(crate) const JSON_BEGIN_FORMAT: &str = "{";
pub(crate) const JSON_END_FORMAT: &str = "}";
pub(crate) const JSON_ARRAY_BEGIN_FORMAT: &str = "[";
pub(crate) const JSON_ARRAY_END_FORMAT: &str = "]";

pub(crate) fn strip_json_fence(input: &str) -> Option<&str> {
    if input.starts_with(TOOL_CALL_BEGIN_JSON) && input.ends_with(TOOL_CALL_CONTAINER) {
        return Some(
            input
                .trim_start_matches(TOOL_CALL_BEGIN_JSON)
                .trim_end_matches(TOOL_CALL_CONTAINER)
                .trim(),
        );
    }
    if input.starts_with(TOOL_CALL_CONTAINER) && input.ends_with(TOOL_CALL_CONTAINER) {
        return Some(
            input
                .trim_start_matches(TOOL_CALL_CONTAINER)
                .trim_end_matches(TOOL_CALL_CONTAINER)
                .trim(),
        );
    }
    if input.starts_with(JSON_BEGIN_FORMAT) && input.ends_with(JSON_END_FORMAT) {
        return Some(input);
    }
    if input.starts_with(JSON_ARRAY_BEGIN_FORMAT) && input.ends_with(JSON_ARRAY_END_FORMAT) {
        return Some(input);
    }
    None
}
