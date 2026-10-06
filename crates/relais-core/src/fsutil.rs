//! Small file helpers shared by every module.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use crate::error::{Error, Result};

/// Writes `bytes` to `path` through a temporary file in the same directory and
/// a rename, so a reader never sees a half-written file. This is how Claude Code
/// itself writes `.claude.json`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::io(path, std::io::Error::other("no parent directory")))?;
    fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{file_name}.relais-{}.tmp", std::process::id()));
    let mut file = fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
    file.write_all(bytes).map_err(|e| Error::io(&tmp, e))?;
    file.sync_all().map_err(|e| Error::io(&tmp, e))?;
    drop(file);
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        Error::io(path, e)
    })
}

/// Serializes `value` with two-space indentation and writes it atomically.
pub fn write_json_atomic(path: &Path, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| Error::Json {
        path: path.into(),
        source: e,
    })?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Reads a JSON file. A missing file yields `Ok(None)`.
pub fn read_json(path: &Path) -> Result<Option<Value>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| Error::Json {
                path: path.into(),
                source: e,
            }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io(path, e)),
    }
}

/// Modification time of `path` in seconds since the Unix epoch, or 0 when unknown.
pub fn modified_secs(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How old Claude Code's lock directory must be before it counts as abandoned
/// (the default of the lockfile library Claude Code uses).
pub const CONFIG_LOCK_STALE_AFTER: Duration = Duration::from_secs(10);

/// The lock Claude Code takes before writing a config file: a directory named
/// `<file>.lock`, created to take it and removed to give it back. Relais takes
/// the same lock, so its writes never interleave with Claude Code's.
#[derive(Debug)]
pub struct ConfigLock {
    dir: PathBuf,
}

impl ConfigLock {
    /// Takes the lock on `target`, waiting up to `wait` for its holder. A lock
    /// older than [`CONFIG_LOCK_STALE_AFTER`] was abandoned and is taken over.
    pub fn acquire(target: &Path, wait: Duration) -> Result<ConfigLock> {
        let mut name = target.as_os_str().to_os_string();
        name.push(".lock");
        let dir = PathBuf::from(name);
        let deadline = Instant::now() + wait;
        loop {
            match fs::create_dir(&dir) {
                Ok(()) => return Ok(ConfigLock { dir }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let abandoned = fs::metadata(&dir)
                        .and_then(|meta| meta.modified())
                        .ok()
                        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                        .is_some_and(|age| age > CONFIG_LOCK_STALE_AFTER);
                    if abandoned && fs::remove_dir(&dir).is_ok() {
                        continue;
                    }
                    if Instant::now() >= deadline {
                        return Err(Error::Locked(target.to_path_buf()));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let parent = dir.parent().ok_or_else(|| Error::io(&dir, e))?;
                    fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
                }
                Err(e) => return Err(Error::io(&dir, e)),
            }
        }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_round_trip_through_atomic_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.json");

        write_json_atomic(&path, &json!({"a": 1})).unwrap();

        assert_eq!(read_json(&path).unwrap(), Some(json!({"a": 1})));
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "no temporary file is left behind");
    }

    #[test]
    fn missing_file_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_json(&dir.path().join("absent.json")).unwrap(), None);
    }

    fn lock_dir(target: &Path) -> PathBuf {
        let mut name = target.as_os_str().to_os_string();
        name.push(".lock");
        PathBuf::from(name)
    }

    #[test]
    fn lock_is_a_directory_removed_on_release() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(".claude.json");
        {
            let _lock = ConfigLock::acquire(&target, Duration::from_millis(100)).unwrap();
            assert!(lock_dir(&target).is_dir());
        }
        assert!(!lock_dir(&target).exists());
    }

    #[test]
    fn a_lock_held_elsewhere_makes_us_wait_then_give_up() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(".claude.json");
        fs::create_dir(lock_dir(&target)).unwrap();

        let started = Instant::now();
        let result = ConfigLock::acquire(&target, Duration::from_millis(200));

        assert!(matches!(result, Err(Error::Locked(_))));
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert!(
            lock_dir(&target).is_dir(),
            "someone else's lock is left alone"
        );
    }

    #[test]
    fn an_abandoned_lock_is_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(".claude.json");
        fs::create_dir(lock_dir(&target)).unwrap();
        let old = SystemTime::now() - Duration::from_secs(60);
        fs::File::open(lock_dir(&target))
            .unwrap()
            .set_modified(old)
            .unwrap();

        let lock = ConfigLock::acquire(&target, Duration::from_millis(100));

        assert!(lock.is_ok());
    }

    #[test]
    fn a_lock_released_while_waiting_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(".claude.json");
        let held = lock_dir(&target);
        fs::create_dir(&held).unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            fs::remove_dir(&held).unwrap();
        });

        let lock = ConfigLock::acquire(&target, Duration::from_secs(2));

        releaser.join().unwrap();
        assert!(lock.is_ok());
    }
}
