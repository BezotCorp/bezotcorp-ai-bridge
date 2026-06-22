mod app_state;
mod logging;
mod normalizer;
mod openai;
mod proxy;
mod sse;

use axum::{
    Router,
    routing::{get, post},
};
use proxy::{chat_completions, models};
use std::{fs, path::PathBuf, sync::Arc};

use crate::app_state::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let state = Arc::new(AppState::new(
        "http://127.0.0.1:11434".to_string(),
        PathBuf::from("logs"),
    ));

    fs::create_dir_all(&state.log_dir).expect("failed to create logs directory");

    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:4188")
        .await
        .expect("failed to bind proxy");

    println!("Kilo Ollama proxy listening on http://127.0.0.1:4188");

    axum::serve(listener, app).await.expect("proxy failed");
}
