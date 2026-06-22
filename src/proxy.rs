use crate::{app_state::AppState, chat_request::ChatRequest, logging::log_bytes};
use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use std::sync::Arc;

pub async fn models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state
        .client
        .get(format!("{}/v1/models", state.ollama_base))
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            let bytes = response.bytes().await.unwrap_or_default();
            (status, bytes).into_response()
        }
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            format!("failed to forward models request: {error}"),
        )
            .into_response(),
    }
}

pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    log_bytes(&state.log_dir, "kilo-request", &body);

    let request: ChatRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("invalid OpenAI chat request: {error}"),
            )
                .into_response();
        }
    };

    match state
        .client
        .post(format!("{}/v1/chat/completions", state.ollama_base))
        .headers(filter_headers(&headers))
        .body(body)
        .send()
        .await
    {
        Ok(response) => handle_ollama_stream(state, request, response).await,
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            format!("failed to forward chat request: {error}"),
        )
            .into_response(),
    }
}

async fn handle_ollama_stream(
    state: Arc<AppState>,
    request: ChatRequest,
    response: reqwest::Response,
) -> Response {
    let status = response.status();

    if !status.is_success() {
        let bytes = response.bytes().await.unwrap_or_default();
        log_bytes(&state.log_dir, "ollama-error", &bytes);
        return (status, bytes).into_response();
    }

    let stream =
        crate::raw_tool_call_scanner::scan_ollama_stream(response, request, state.log_dir.clone());

    Response::builder()
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|error| {
            eprintln!("failed to build streaming response: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to build streaming response",
            )
                .into_response()
        })
}

fn filter_headers(headers: &HeaderMap) -> reqwest::header::HeaderMap {
    let mut forwarded = reqwest::header::HeaderMap::new();

    for (name, value) in headers.iter() {
        let lower = name.as_str().to_ascii_lowercase();

        if matches!(
            lower.as_str(),
            "host" | "content-length" | "connection" | "accept-encoding"
        ) {
            continue;
        }

        if let (Ok(header_name), Ok(header_value)) = (
            reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            forwarded.insert(header_name, header_value);
        }
    }

    forwarded
}
