//! Persistent desktop log.
//!
//! Desktop Rust code historically reported failures with `eprintln!`, which a
//! bundled app sends nowhere a user can find. `tracing` events are written to
//! `desktop.log` in the platform log directory
//! (`~/Library/Logs/<bundle identifier>/` on macOS), so a failed agent
//! operation leaves evidence behind. The file is bounded: once it passes
//! [`MAX_LOG_BYTES`] at startup it is rotated to `desktop.log.1`, replacing
//! the previous rotation.

use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Rotation threshold checked at startup.
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const LOG_FILE: &str = "desktop.log";

/// Rotate `dir/desktop.log` when it is too large, then open it for append.
pub fn open_log_file(dir: &Path) -> std::io::Result<(PathBuf, File)> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(LOG_FILE);
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES) {
        std::fs::rename(&path, dir.join(format!("{LOG_FILE}.1")))?;
    }
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    Ok((path, file))
}

/// Install the global `tracing` subscriber writing to the desktop log.
///
/// Returns the log path. Failing to open the file is reported, not fatal:
/// the app still runs, it just keeps no log.
pub fn init(dir: &Path) -> Result<PathBuf, String> {
    let (path, file) = open_log_file(dir)
        .map_err(|error| format!("could not open desktop log in {}: {error}", dir.display()))?;
    use tracing_subscriber::{layer::SubscriberExt, Layer};
    // Only the desktop's own events: embedded libraries (mesh-llm) log at
    // volume and would grow the file between startup rotations.
    let own_events = tracing_subscriber::filter::Targets::new()
        .with_target(env!("CARGO_CRATE_NAME"), tracing::Level::INFO);
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(Mutex::new(file))
            .with_ansi(false)
            .with_filter(own_events),
    );
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|error| format!("could not install the desktop log: {error}"))?;
    tracing::info!("desktop log opened");
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_log_is_rotated_once_and_a_fresh_file_opened() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LOG_FILE);
        std::fs::write(&path, vec![b'x'; (MAX_LOG_BYTES + 1) as usize]).unwrap();
        let (opened, _file) = open_log_file(dir.path()).unwrap();
        assert_eq!(opened, path);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        assert_eq!(
            std::fs::metadata(dir.path().join("desktop.log.1"))
                .unwrap()
                .len(),
            MAX_LOG_BYTES + 1
        );
    }

    #[test]
    fn small_log_is_appended_to() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LOG_FILE);
        std::fs::write(&path, b"earlier\n").unwrap();
        let (_, mut file) = open_log_file(dir.path()).unwrap();
        std::io::Write::write_all(&mut file, b"later\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "earlier\nlater\n");
        assert!(!dir.path().join("desktop.log.1").exists());
    }
}
