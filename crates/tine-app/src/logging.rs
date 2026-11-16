//! Logging: `tracing` to a size-rotated file sink in `%LOCALAPPDATA%\Tine\logs`
//! (engineering standard §6: 7 files × 2 MB).
//!
//! `tracing-appender` only offers time-based rotation, so this module carries
//! a small in-house size-rotating writer (`2 MB` per file; rolling
//! `tine.log` → `tine.1.log` → … → `tine.7.log`, deleting the oldest) —
//! the repo's "small, well-understood code ourselves" policy. Nothing on any
//! RT path logs: the app only logs control-plane events on the shell, pipe,
//! and controller threads.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Bytes per log file before rotation.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// How many log files exist at once (`tine.log` + `tine.1..=6.log`).
pub const KEEP: u32 = 7;

/// The active log file with size-based rollover.
pub struct RotatingFile {
    dir: PathBuf,
    file: Option<File>,
    written: u64,
    max_bytes: u64,
    keep: u32,
}

impl RotatingFile {
    /// Opens (creating if needed) `dir/tine.log` with the standard limits.
    ///
    /// # Errors
    /// Propagates filesystem errors from creating the directory or opening
    /// the log file. Logging is best-effort for the app: callers that get an
    /// `Err` run on without a file sink.
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let base = dir.join("tine.log");
        prune_dir(dir, KEEP);
        let file = open_append(&base)?;
        let written = file.metadata().map_or(0, |m| m.len());
        Ok(Self {
            dir: dir.to_owned(),
            file: Some(file),
            written,
            max_bytes: MAX_FILE_BYTES,
            keep: KEEP,
        })
    }

    /// Test constructor with a tiny limit.
    #[cfg(test)]
    pub fn with_limits(dir: &Path, max_bytes: u64, keep: u32) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            dir: dir.to_owned(),
            file: open_append(&dir.join("tine.log")).ok(),
            written: 0,
            max_bytes,
            keep,
        })
    }

    /// Rolls `tine.log` → `tine.1.log` → … → `tine.<keep-1>.log`, deleting
    /// the oldest generation, then reopens a fresh `tine.log`.
    fn rotate(&mut self) {
        self.file.take();
        let base = self.dir.join("tine.log");
        // Generation k becomes k+1; drop the oldest slot first so the budget
        // is always `keep` files (generation 0 = tine.log … keep-1).
        let _ = std::fs::remove_file(self.dir.join(rolled_name(self.keep - 1)));
        for n in (1..self.keep - 1).rev() {
            let from = self.dir.join(rolled_name(n));
            let to = self.dir.join(rolled_name(n + 1));
            if from.exists() {
                let _ = std::fs::rename(&from, &to);
            }
        }
        let _ = std::fs::rename(&base, self.dir.join(rolled_name(1)));
        match open_append(&base) {
            Ok(f) => {
                self.file = Some(f);
                self.written = 0;
            }
            Err(_) => self.file = None,
        }
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Rotate *before* the active file would pass the cap, so a single
        // large line never overflows into the next generation by half.
        if self.written > 0 && self.written.saturating_add(buf.len() as u64) > self.max_bytes {
            self.rotate();
        }
        let file = match &mut self.file {
            Some(f) => f,
            None => self.file.insert(open_append(&self.dir.join("tine.log"))?),
        };
        file.write_all(buf)?;
        self.written = self.written.saturating_add(buf.len() as u64);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

fn rolled_name(n: u32) -> String {
    format!("tine.{n}.log")
}

fn open_append(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

/// Keeps at most `keep` log files (generation `0` = `tine.log` is newest).
fn prune_dir(dir: &Path, keep: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<(u32, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name == "tine.log" {
                Some((0u32, e.path()))
            } else {
                let n = name
                    .strip_prefix("tine.")
                    .and_then(|s| s.strip_suffix(".log"))
                    .and_then(|s| s.parse::<u32>().ok())?;
                Some((n, e.path()))
            }
        })
        .collect();
    logs.sort_by_key(|(n, _)| *n);
    // Generation 0 is the live file; only generations >= keep - 1 remaining
    // after a roll would exceed the budget.
    let max_gen = keep.saturating_sub(1);
    for (n, path) in logs {
        if n > max_gen {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Shared sink handle installed as the `tracing` global write target. The
/// `Mutex` is taken once per log line on a control-plane thread; no RT path
/// logs. Cloning the handle shares the same file (one `Arc` bump per line).
#[derive(Clone)]
pub struct LogSink {
    inner: Arc<Mutex<RotatingFile>>,
}

impl LogSink {
    fn open(dir: &Path) -> std::io::Result<Self> {
        Ok(Self {
            inner: Arc::new(Mutex::new(RotatingFile::open(dir)?)),
        })
    }
}

impl Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.inner.lock() {
            Ok(mut file) => file.write(buf),
            // A panicking writer thread must not stop logging.
            Err(poisoned) => poisoned.into_inner().write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.inner.lock() {
            Ok(mut file) => file.flush(),
            Err(poisoned) => poisoned.into_inner().flush(),
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = LogSink;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Installs the global `tracing` subscriber writing to the rotating file.
///
/// # Errors
/// Returns the io error when the log directory/file cannot be created; in
/// that case nothing is installed and the app continues without a file sink.
pub fn init(dir: &Path) -> std::io::Result<()> {
    let sink = LogSink::open(dir)?;
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_target(true)
        .with_level(true)
        .with_writer(sink)
        .try_init();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{KEEP, MAX_FILE_BYTES, RotatingFile, rolled_name};
    use std::io::Write;
    use std::path::Path;

    fn temp(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tine-log-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn rotates_at_max_and_deletes_oldest() {
        let dir = temp("rotate");
        let mut f = RotatingFile::with_limits(&dir, 120, 4).expect("open");
        let line = format!("{}\n", "x".repeat(100));
        for _ in 0..60 {
            f.write_all(line.as_bytes()).expect("write");
        }
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .expect("read_dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        // Cap: tine.log + tine.1.log + tine.2.log + tine.3.log.
        assert!(names.len() <= 4, "files: {names:?}");
        assert!(names.contains(&"tine.log".to_owned()));
        assert!(!names.contains(&rolled_name(4)), "oldest must be deleted");
    }

    #[test]
    fn first_write_then_rollover_keeps_tail() {
        let dir = temp("tail");
        let mut f = RotatingFile::with_limits(&dir, 50, 3).expect("open");
        f.write_all(b"short line\n").expect("write");
        // One `write_all` of the whole oversized line, like tracing emits it.
        let mut long_line = vec![b'y'; 200];
        long_line.push(b'\n');
        f.write_all(&long_line).expect("write");
        let content = std::fs::read_to_string(dir.join("tine.log")).unwrap_or_default();
        assert!(content.starts_with('y'), "fresh file holds the long line");
        assert!(dir.join(rolled_name(1)).exists(), "previous file rolled");
    }

    #[test]
    fn standard_limits_match_spec() {
        assert_eq!(MAX_FILE_BYTES, 2 * 1024 * 1024);
        assert_eq!(KEEP, 7);
        assert!(Path::new("tine.log").is_relative());
    }
}
