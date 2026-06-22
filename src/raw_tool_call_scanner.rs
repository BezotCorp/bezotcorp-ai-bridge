use crate::{
    chat_request::ChatRequest,
    normalizer::{TOOL_CALL_BEGIN_JSON, strip_json_fence},
    openai::ToolCallChunk,
    sse::{INACTIVITY_TIMEOUT, RAW_TOOL_CALL_PASSTHROUGH_THRESHOLD, collect_sse_content},
};
use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::Response;
use serde_json::Value;
use std::{convert::Infallible, mem, path::PathBuf};
use tokio::time::timeout;

enum ScanState {
    Scanning,
    MaybeFence,
    Candidate,
}

pub fn scan_ollama_stream(
    response: Response,
    request: ChatRequest,
    log_dir: PathBuf,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    async_stream::stream! {
        let mut upstream = response.bytes_stream();

        let mut raw = Vec::new();
        let mut line_buf: Vec<u8> = Vec::new();
        let mut content = String::new();
        let mut first_chunk: Option<Value> = None;
        let mut usage_chunk: Option<Value> = None;

        let mut pending_raw: Vec<u8> = Vec::new();
        let mut released_len: usize = 0;
        let mut marker_start: usize = 0;
        let mut state = ScanState::Scanning;

        loop {
            let next = match timeout(INACTIVITY_TIMEOUT, upstream.next()).await {
                Ok(next) => next,
                Err(_) => {
                    crate::logging::log_bytes(&log_dir, "ollama-timeout", &raw);

                    if !pending_raw.is_empty() {
                        yield Ok::<_, Infallible>(Bytes::from(mem::take(&mut pending_raw)));
                    }

                    yield Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n"));
                    return;
                }
            };

            let Some(item) = next else {
                break;
            };

            let Ok(bytes) = item else {
                crate::logging::log_bytes(&log_dir, "ollama-stream-error", &raw);

                if !pending_raw.is_empty() {
                    yield Ok::<_, Infallible>(Bytes::from(mem::take(&mut pending_raw)));
                }

                yield Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n"));
                return;
            };

            raw.extend_from_slice(&bytes);
            pending_raw.extend_from_slice(&bytes);
            line_buf.extend_from_slice(&bytes);

            if let Some(last_newline) = line_buf.iter().rposition(|&b| b == b'\n') {
                let complete: Vec<u8> = line_buf.drain(..=last_newline).collect();

                collect_sse_content(
                    &complete,
                    &mut content,
                    &mut first_chunk,
                    &mut usage_chunk,
                );
            }

            loop {
                match state {
                    ScanState::Scanning => {
                        let unscanned = &content[released_len..];

                        match unscanned.find(|c: char| c == '`' || c == '{' || c == '[') {
                            None => {
                                if !pending_raw.is_empty() {
                                    yield Ok::<_, Infallible>(Bytes::from(mem::take(&mut pending_raw)));
                                }

                                released_len = content.len();
                                break;
                            }
                            Some(offset) => {
                                marker_start = released_len + offset;

                                state = if matches!(content.as_bytes()[marker_start], b'{' | b'[') {
                                    ScanState::Candidate
                                } else {
                                    ScanState::MaybeFence
                                };
                            }
                        }
                    }

                    ScanState::MaybeFence => {
                        let attempt = &content[marker_start..];

                        if attempt.starts_with(TOOL_CALL_BEGIN_JSON) {
                            state = ScanState::Candidate;
                        } else if TOOL_CALL_BEGIN_JSON.starts_with(attempt) {
                            break;
                        } else {
                            if !pending_raw.is_empty() {
                                yield Ok::<_, Infallible>(Bytes::from(mem::take(&mut pending_raw)));
                            }

                            released_len = content.len();
                            state = ScanState::Scanning;
                            break;
                        }
                    }

                    ScanState::Candidate => {
                        let candidate = &content[marker_start..];

                        if let Some(tool_calls) =
                            ToolCallChunk::try_normalize_raw_tool_call(candidate, &request)
                        {
                            let sse = crate::sse::build_tool_call_sse(
                                &request.model,
                                first_chunk.clone(),
                                usage_chunk.clone(),
                                tool_calls,
                            );

                            crate::logging::log_bytes(
                                &log_dir,
                                "proxy-normalized-tool-call",
                                sse.as_bytes(),
                            );
                            crate::logging::log_bytes(&log_dir, "ollama-response-stream", &raw);

                            yield Ok::<_, Infallible>(Bytes::from(sse));
                            return;
                        }

                        let fence_closed = strip_json_fence(candidate).is_some();
                        let too_long = candidate.len() > RAW_TOOL_CALL_PASSTHROUGH_THRESHOLD;

                        if fence_closed || too_long {
                            if !pending_raw.is_empty() {
                                yield Ok::<_, Infallible>(Bytes::from(mem::take(&mut pending_raw)));
                            }

                            released_len = content.len();
                            state = ScanState::Scanning;
                            break;
                        }

                        break;
                    }
                }
            }
        }

        crate::logging::log_bytes(&log_dir, "ollama-response-stream", &raw);

        if !pending_raw.is_empty() {
            yield Ok::<_, Infallible>(Bytes::from(pending_raw));
        }
    }
}
