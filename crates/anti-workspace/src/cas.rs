//! CAS write (maestro fs.rs:120-141) — write-if-unchanged + lock marker.
//! Chống last-writer-wins giữa 2 peers cùng sửa 1 file.

use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CasError {
    #[error("file changed since baseline (expected sha {expected}, found {found})")]
    Changed { path: PathBuf, expected: String, found: String },
    #[error("lock held by {holder}")]
    LockHeld { path: PathBuf, holder: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct Baseline {
    pub sha256: String,
}

pub fn sha256_of(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}

/// Baseline = sha256 của file tại thời điểm peer bắt đầu sửa.
pub fn read_baseline(path: &Path) -> std::io::Result<Baseline> {
    Ok(Baseline { sha256: sha256_of(path)? })
}

/// Ghi chỉ khi file vẫn còn đúng baseline. Ngược lại → CasError::Changed.
pub fn write_if_unchanged(path: &Path, content: &str, base: &Baseline) -> Result<(), CasError> {
    if path.exists() {
        let now = sha256_of(path)?;
        if now != base.sha256 {
            return Err(CasError::Changed {
                path: path.to_path_buf(),
                expected: base.sha256.clone(),
                found: now,
            });
        }
    }
    std::fs::write(path, content)?;
    Ok(())
}

/// Lock marker: `.anti.lock` chứa holder. Atomic create_new — không bao giờ
/// overwrite lock của peer khác.
pub fn acquire_lock(dir: &Path, holder: &str) -> Result<(), CasError> {
    let lock = dir.join(".anti.lock");
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
        Ok(mut f) => {
            use std::io::Write;
            let _ = f.write_all(holder.as_bytes());
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let holder_now = std::fs::read_to_string(&lock).unwrap_or_default();
            Err(CasError::LockHeld { path: lock, holder: holder_now })
        }
        Err(e) => Err(CasError::Io(e)),
    }
}

pub fn release_lock(dir: &Path) -> std::io::Result<()> {
    let lock = dir.join(".anti.lock");
    if lock.exists() {
        std::fs::remove_file(lock)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_succeeds_when_unchanged() {
        let dir = std::env::temp_dir().join(format!("anti-cas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.rs");
        std::fs::write(&f, "v1").unwrap();
        let base = read_baseline(&f).unwrap();
        assert!(write_if_unchanged(&f, "v2", &base).is_ok());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "v2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_fails_when_changed_by_other() {
        let dir = std::env::temp_dir().join(format!("anti-cas2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.rs");
        std::fs::write(&f, "v1").unwrap();
        let base = read_baseline(&f).unwrap();
        std::fs::write(&f, "v1.5").unwrap(); // peer khác sửa
        assert!(matches!(
            write_if_unchanged(&f, "v2", &base),
            Err(CasError::Changed { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lock_acquire_release() {
        let dir = std::env::temp_dir().join(format!("anti-cas-lock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(acquire_lock(&dir, "peer-1").is_ok());
        assert!(matches!(
            acquire_lock(&dir, "peer-2"),
            Err(CasError::LockHeld { .. })
        ));
        release_lock(&dir).unwrap();
        assert!(acquire_lock(&dir, "peer-2").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
