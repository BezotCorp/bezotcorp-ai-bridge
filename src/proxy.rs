use crate::normalizer::{JSON_BEGIN_FORMAT, TOOL_CALL_BEGIN_JSON, TOOL_CALL_CONTAINER};
use crate::{
    app_state::AppState,
    chat_reques::ChatRequest,
    logging::log_bytes,
    normalizer::strip_json_fence,
    openai::ToolCallChunk,
    sse::{
        INACTIVITY_TIMEOUT, RAW_TOOL_CALL_PASSTHROUGH_THRESHOLD, build_tool_call_sse,
        collect_sse_content,
    },
};
use axum::{
    body::{Body, Bytes},
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use serde_json::Value;
use std::{convert::Infallible, mem, sync::Arc};
use tokio::time::timeout;

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

    let log_dir = state.log_dir.clone();

    let stream = async_stream::stream! {
        let mut upstream = response.bytes_stream();

        let mut raw = Vec::new();
        let mut buffered_sse = Vec::new();
        let mut line_buf: Vec<u8> = Vec::new();
        let mut content = String::new();
        let mut first_chunk: Option<Value> = None;
        let mut usage_chunk: Option<Value> = None;

        let mut maybe_raw_json_tool_call = false;
        let mut passthrough = false;

        loop {
            let next = match timeout(INACTIVITY_TIMEOUT, upstream.next()).await {
                Ok(next) => next,
                Err(_) => {
                    log_bytes(&log_dir, "ollama-timeout", &raw);
                    if !buffered_sse.is_empty() {
                        yield Ok::<_, Infallible>(Bytes::from(mem::take(&mut buffered_sse)));
                    }
                    yield Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n"));
                    return;
                }
            };

            let Some(item) = next else {
                break;
            };

            let Ok(bytes) = item else {
                log_bytes(&log_dir, "ollama-stream-error", &raw);
                if !buffered_sse.is_empty() {
                    yield Ok::<_, Infallible>(Bytes::from(mem::take(&mut buffered_sse)));
                }
                yield Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n"));
                return;
            };

            raw.extend_from_slice(&bytes);

            if passthrough {
                yield Ok::<_, Infallible>(bytes);
                continue;
            }

            buffered_sse.extend_from_slice(&bytes);
            line_buf.extend_from_slice(&bytes);

            if let Some(last_newline) = line_buf.iter().rposition(|&b| b == b'\n') {
                let complete: Vec<u8> = line_buf.drain(..=last_newline).collect();
                collect_sse_content(&complete, &mut content, &mut first_chunk, &mut usage_chunk);
            }

            let trimmed = content.trim_start();

            if !maybe_raw_json_tool_call {
                if trimmed.starts_with(TOOL_CALL_BEGIN_JSON) || trimmed.starts_with(JSON_BEGIN_FORMAT) {
                    maybe_raw_json_tool_call = true;
                } else if trimmed.starts_with(TOOL_CALL_CONTAINER) {
                    if !TOOL_CALL_BEGIN_JSON.starts_with(trimmed) {
                        passthrough = true;
                        yield Ok::<_, Infallible>(Bytes::from(mem::take(&mut buffered_sse)));
                        continue;
                    }
                } else if !trimmed.is_empty() {
                    passthrough = true;
                    yield Ok::<_, Infallible>(Bytes::from(mem::take(&mut buffered_sse)));
                    continue;
                }
            }

            if maybe_raw_json_tool_call {
                if let Some(tool_call) = ToolCallChunk::try_normalize_raw_tool_call(&content, &request) {
                    let sse = build_tool_call_sse(
                        &request.model,
                        first_chunk.clone(),
                        usage_chunk.clone(),
                        tool_call,
                    );

                    log_bytes(&log_dir, "proxy-normalized-tool-call", sse.as_bytes());
                    log_bytes(&log_dir, "ollama-response-stream", &raw);

                    yield Ok::<_, Infallible>(Bytes::from(sse));
                    return;
                }

                if strip_json_fence(trimmed).is_some() {
                    passthrough = true;
                    yield Ok::<_, Infallible>(Bytes::from(mem::take(&mut buffered_sse)));
                    continue;
                }

                if content.len() > RAW_TOOL_CALL_PASSTHROUGH_THRESHOLD {
                    passthrough = true;
                    yield Ok::<_, Infallible>(Bytes::from(mem::take(&mut buffered_sse)));
                    continue;
                }
            }
        }

        log_bytes(&log_dir, "ollama-response-stream", &raw);

        if !buffered_sse.is_empty() {
            yield Ok::<_, Infallible>(Bytes::from(buffered_sse));
        }
    };

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
