use std::path::Path;

use cabin_fs::write_atomic;

use crate::error::RegistryError;

/// Atomically replace `path` with `body`, tagging any IO error
/// with `path` so callers can point users at the destination
/// that failed. The parent directory must already exist; callers
/// create it explicitly so directory-boundary failures surface
/// before the atomic write is attempted.
pub(crate) fn atomically_write(path: &Path, body: &[u8]) -> Result<(), RegistryError> {
    write_atomic(path, body).map_err(|source| RegistryError::Io {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;

    #[test]
    fn replaces_existing_contents() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, b"stale").unwrap();
        atomically_write(&path, b"fresh").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fresh");
    }

    #[test]
    fn reports_destination_when_parent_directory_missing() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nonexistent").join("config.json");
        let err = atomically_write(&missing, b"x").unwrap_err();
        match err {
            RegistryError::Io { path, .. } => assert_eq!(path, missing),
            other => panic!("expected RegistryError::Io, got {other:?}"),
        }
    }
}
