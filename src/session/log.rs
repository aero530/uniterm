//! Log received bytes to a file.
//!
//! Raw bytes, not rendered output. The Tauri build logged the *formatted* text, so a
//! hex-mode session wrote hex to disk and the log's content depended on a display setting.
//! Raw bytes are lossless and can be replayed through any view.

use std::path::PathBuf;

use tokio::io::AsyncWriteExt;
use tracing::warn;

/// A log file, or nothing.
pub struct Logger {
    file: Option<tokio::fs::File>,
}

impl Logger {
    /// Open `path` for appending. Failure is reported once and then disabled.
    pub async fn open(path: Option<PathBuf>) -> (Self, Option<String>) {
        let Some(path) = path else {
            return (Self { file: None }, None);
        };
        match tokio::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .await
        {
            Ok(file) => (Self { file: Some(file) }, None),
            Err(e) => {
                warn!("could not open log file {}: {e}", path.display());
                (
                    Self { file: None },
                    Some(format!("Could not open log file {}: {e}", path.display())),
                )
            }
        }
    }

    /// Append bytes. Returns a message on the first failure, then stops trying.
    pub async fn write(&mut self, data: &[u8]) -> Option<String> {
        let file = self.file.as_mut()?;
        if let Err(e) = file.write_all(data).await {
            // Stop after the first failure rather than reporting once per read.
            self.file = None;
            return Some(format!("Log write failed: {e}"));
        }
        None
    }

    pub async fn flush(&mut self) {
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush().await;
        }
    }
}
