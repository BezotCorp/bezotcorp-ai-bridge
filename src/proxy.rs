use crate::{
    app_state::AppState,
    chat_request::ChatRequest,
    logging::log_bytes,
    normalizer::{TOOL_CALL_BEGIN_JSON, strip_json_fence},
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

/// État du scanner de tool-call brut dans le texte de l'assistant.
///
/// Contrairement à une détection figée au tout premier caractère, ce
/// scanner peut se réarmer plusieurs fois dans le même flux : un backtick
/// isolé (ex. `` `read` `` en style markdown) peut s'avérer une fausse
/// alerte sans bloquer le reste du message.
enum ScanState {
    /// Rien de suspect en attente : tout texte reçu est relâché dès qu'on
    /// confirme l'absence de marqueur.
    Scanning,
    /// Un backtick a été vu et reste, pour l'instant, un préfixe valide
    /// de "```json" — pas encore assez de caractères pour trancher.
    MaybeFence,
    /// Candidat confirmé (objet JSON nu, ou fence "```json" ouverte) : on
    /// attend la fermeture pour normaliser ou abandonner.
    Candidate,
}

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
                let mut line_buf: Vec<u8> = Vec::new();
                let mut content = String::new();
                let mut first_chunk: Option<Value> = None;
                let mut usage_chunk: Option<Value> = None;

                // Octets SSE bruts reçus depuis le dernier point relâché comme texte
                // confirmé sûr. Relâché immédiatement en `Scanning` tant qu'aucun
                // marqueur n'apparaît ; retenu pendant `MaybeFence`/`Candidate`.
                let mut pending_raw: Vec<u8> = Vec::new();
                // Longueur de `content` déjà relâchée au client.
                let mut released_len: usize = 0;
                // Position dans `content` où le marqueur actuel (backtick ou `{`)
                // a été repéré. Signifiant uniquement hors de `Scanning`.
                let mut marker_start: usize = 0;
                let mut state = ScanState::Scanning;

                loop {
                    let next = match timeout(INACTIVITY_TIMEOUT, upstream.next()).await {
                        Ok(next) => next,
                        Err(_) => {
                            log_bytes(&log_dir, "ollama-timeout", &raw);
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
                        log_bytes(&log_dir, "ollama-stream-error", &raw);
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
                        collect_sse_content(&complete, &mut content, &mut first_chunk, &mut usage_chunk);
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
                                        // On retombe dans ce `match` pour analyser
                                        // immédiatement ce nouveau marqueur, sans
                                        // attendre le prochain octet reçu.
                                    }
                                }
                            }
                            ScanState::MaybeFence => {
                                let attempt = &content[marker_start..];

                                if attempt.starts_with(TOOL_CALL_BEGIN_JSON) {
                                    state = ScanState::Candidate;
                                } else if TOOL_CALL_BEGIN_JSON.starts_with(attempt) {
                                    // Toujours un préfixe valide de "```json"
                                    // (ex: "`", "``", "```", "```j"...) : on attend
                                    // la suite avant de trancher.
                                    break;
                                } else {
                                    // Diverge ("```rust", ou un simple backtick de
                                    // code inline comme `` `read` ``) : ce n'était
                                    // pas une fence JSON. On relâche tout ce qui
                                    // était retenu et on reprend la veille.
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
        let sse = build_tool_call_sse(
            &request.model,
            first_chunk.clone(),
            usage_chunk.clone(),
            tool_calls,
        );

                                    log_bytes(&log_dir, "proxy-normalized-tool-call", sse.as_bytes());
                                    log_bytes(&log_dir, "ollama-response-stream", &raw);

                                    // Le préambule éventuel ("I will call the read
                                    // tool...") n'est jamais relâché : il est
                                    // remplacé par le tool_calls structuré.
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

                log_bytes(&log_dir, "ollama-response-stream", &raw);

                if !pending_raw.is_empty() {
                    yield Ok::<_, Infallible>(Bytes::from(pending_raw));
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
