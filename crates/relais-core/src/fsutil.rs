//! Small file helpers shared by every module.

use std::fs;
use std::io::Write;
use std::path::Path;

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
}
