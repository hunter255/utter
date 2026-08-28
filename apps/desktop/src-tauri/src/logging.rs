//! Bounded persistent logging for GUI builds whose stderr is normally
//! invisible. Every line is redacted before it reaches disk.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use regex::Regex;

const LOG_FILE: &str = "utter.log";
const LOG_BYTES: u64 = 1024 * 1024;
const LOG_FILES: usize = 4;
const LOGGING_FALLBACK_NOTICE: &str =
    "Persistent logs are unavailable; Utter is continuing with console logging only.";

static URL_QUERY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(https?://[^\s\"'<>?]+)\?[^\s\"'<>]*"#).expect("valid URL regex")
});
static CREDENTIAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)([\"']?(?:authorization|api[_-]?key|access[_-]?token|token|secret|password)[\"']?\s*[:=]\s*)[\"']?(?:bearer\s+)?[^\s,\"'}]+"#,
    )
    .expect("valid credential regex")
});

pub(crate) fn logs_dir() -> Result<PathBuf, String> {
    utter_store::data_dir()
        .map(|path| path.join("logs"))
        .map_err(|error| error.to_string())
}

/// Builds the one writer installed into `tracing`. Directory failures never
/// abort startup: the returned writer falls back to stderr and the caller
/// receives a safe user-facing notice.
pub(crate) fn log_writer() -> (Mutex<SafeLogWriter>, Option<String>) {
    prepare_log_writer(logs_dir(), LOG_BYTES, LOG_FILES, cfg!(debug_assertions))
}

fn prepare_log_writer(
    dir: Result<PathBuf, String>,
    max_bytes: u64,
    max_files: usize,
    mirror_stderr: bool,
) -> (Mutex<SafeLogWriter>, Option<String>) {
    let result = dir.and_then(|dir| {
        SafeLogWriter::new(&dir, max_bytes, max_files, mirror_stderr)
            .map_err(|error| error.to_string())
    });
    match result {
        Ok(writer) => (Mutex::new(writer), None),
        Err(error) => {
            eprintln!("persistent logging unavailable: {error}");
            (
                Mutex::new(SafeLogWriter::stderr_only()),
                Some(LOGGING_FALLBACK_NOTICE.to_string()),
            )
        }
    }
}

/// Line-buffered so redaction sees a complete tracing event even when the
/// formatter emits it through several `Write` calls.
pub(crate) struct SafeLogWriter {
    file: Option<RollingFile>,
    pending: Vec<u8>,
    mirror_stderr: bool,
    home: Option<String>,
}

impl SafeLogWriter {
    fn new(dir: &Path, max_bytes: u64, max_files: usize, mirror_stderr: bool) -> io::Result<Self> {
        Ok(Self {
            file: Some(RollingFile::new(dir, max_bytes, max_files)?),
            pending: Vec::new(),
            mirror_stderr,
            home: user_home(),
        })
    }

    fn stderr_only() -> Self {
        Self {
            file: None,
            pending: Vec::new(),
            mirror_stderr: true,
            home: user_home(),
        }
    }

    fn emit(&mut self, bytes: &[u8]) {
        let safe = redact(&String::from_utf8_lossy(bytes), self.home.as_deref());
        let write_failed = self
            .file
            .as_mut()
            .is_some_and(|file| file.write_all(safe.as_bytes()).is_err());
        if write_failed {
            self.file = None;
        }
        if self.mirror_stderr || self.file.is_none() {
            let _ = io::stderr().write_all(safe.as_bytes());
        }
    }
}

impl Write for SafeLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line = self.pending.drain(..=end).collect::<Vec<_>>();
            self.emit(&line);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.emit(&line);
        }
        if let Some(file) = self.file.as_mut() {
            file.flush()?;
        }
        io::stderr().flush()
    }
}

struct RollingFile {
    dir: PathBuf,
    file: Option<File>,
    len: u64,
    max_bytes: u64,
    max_files: usize,
}

impl RollingFile {
    fn new(dir: &Path, max_bytes: u64, max_files: usize) -> io::Result<Self> {
        if max_bytes == 0 || max_files == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log bounds must be non-zero",
            ));
        }
        fs::create_dir_all(dir)?;
        let current = dir.join(LOG_FILE);
        let len = current.metadata().map(|meta| meta.len()).unwrap_or(0);
        let mut writer = Self {
            dir: dir.to_path_buf(),
            file: None,
            len,
            max_bytes,
            max_files,
        };
        if len >= max_bytes {
            writer.rotate()?;
        } else {
            writer.open_current()?;
        }
        Ok(writer)
    }

    fn rotated_path(&self, suffix: usize) -> PathBuf {
        self.dir.join(format!("{LOG_FILE}.{suffix}"))
    }

    fn open_current(&mut self) -> io::Result<()> {
        let path = self.dir.join(LOG_FILE);
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        self.len = file.metadata()?.len();
        self.file = Some(file);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.take();
        if self.max_files == 1 {
            remove_if_exists(&self.dir.join(LOG_FILE))?;
        } else {
            remove_if_exists(&self.rotated_path(self.max_files - 1))?;
            for suffix in (1..self.max_files - 1).rev() {
                rename_if_exists(&self.rotated_path(suffix), &self.rotated_path(suffix + 1))?;
            }
            rename_if_exists(&self.dir.join(LOG_FILE), &self.rotated_path(1))?;
        }
        self.open_current()
    }
}

impl Write for RollingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.len > 0 && self.len.saturating_add(buf.len() as u64) > self.max_bytes {
            self.rotate()?;
        }
        self.file
            .as_mut()
            .expect("rolling file is open")
            .write_all(buf)?;
        self.len = self.len.saturating_add(buf.len() as u64);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().expect("rolling file is open").flush()
    }
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn rename_if_exists(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn user_home() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok())
        .filter(|path| !path.is_empty())
}

fn redact(line: &str, home: Option<&str>) -> String {
    let without_query = URL_QUERY.replace_all(line, "$1?[REDACTED]");
    let without_credentials = CREDENTIAL.replace_all(&without_query, "$1[REDACTED]");
    match home {
        Some(home) if !home.is_empty() => without_credentials.replace(home, "<home>"),
        _ => without_credentials.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_keeps_only_the_configured_number_of_bounded_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = SafeLogWriter::new(dir.path(), 32, 3, false).unwrap();
        for index in 0..12 {
            writeln!(writer, "event-{index:02}-safe").unwrap();
        }
        writer.flush().unwrap();

        let files = fs::read_dir(dir.path()).unwrap().collect::<Vec<_>>();
        assert!(files.len() <= 3);
        for entry in files {
            assert!(entry.unwrap().metadata().unwrap().len() <= 32);
        }
    }

    #[test]
    fn persisted_lines_redact_queries_credentials_and_home_paths() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = SafeLogWriter::new(dir.path(), 1024, 2, false).unwrap();
        writer.home = Some("/Users/alice".to_string());
        let line = "GET https://host/model?token=url-secret Authorization: Bearer auth-secret \
                    api_key=key-secret /Users/alice/private/model.bin\n";
        writer.write_all(line.as_bytes()).unwrap();
        writer.flush().unwrap();
        let safe = fs::read_to_string(dir.path().join(LOG_FILE)).unwrap();

        assert!(!safe.contains("url-secret"));
        assert!(!safe.contains("auth-secret"));
        assert!(!safe.contains("key-secret"));
        assert!(!safe.contains("/Users/alice"));
        assert!(safe.contains("?[REDACTED]"));
        assert!(safe.contains("<home>"));
    }

    #[test]
    fn unwritable_log_location_has_a_safe_stderr_fallback_notice() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-directory");
        fs::write(&blocker, "file").unwrap();

        let (writer, notice) = prepare_log_writer(Ok(blocker.join("logs")), 32, 2, false);
        assert_eq!(notice.as_deref(), Some(LOGGING_FALLBACK_NOTICE));
        assert!(!notice
            .unwrap()
            .contains(&blocker.to_string_lossy().to_string()));
        let fallback = writer.lock().unwrap();
        assert!(fallback.file.is_none());
        assert!(fallback.mirror_stderr);
    }
}
