use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn log_bytes(log_dir: &Path, label: &str, bytes: &[u8]) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_millis();

    let path = log_dir.join(format!("{millis}-{label}.json"));

    if let Err(error) = fs::write(&path, bytes) {
        eprintln!("failed to write log {}: {error}", path.display());
    }
}
