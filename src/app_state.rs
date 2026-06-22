use std::path::PathBuf;

#[derive(Clone)]
pub struct AppState {
    pub ollama_base: String,
    pub log_dir: PathBuf,
    pub client: reqwest::Client,
}

impl AppState {
    pub fn new(ollama_base: String, log_dir: PathBuf) -> Self {
        Self {
            ollama_base,
            log_dir,
            client: reqwest::Client::new(),
        }
    }
}
