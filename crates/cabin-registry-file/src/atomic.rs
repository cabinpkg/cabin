use std::io::Write as _;
use std::path::Path;

use atomic_write_file::AtomicWriteFile;

use crate::error::RegistryError;

/// Stage `body` in a sibling temporary file and rename it onto
/// `path` only after a successful write. An interrupted run leaves
/// the previous contents of `path` (if any) in place. The parent
/// directory must already exist; callers create it explicitly so
/// failures surface at the directory boundary rather than masked
/// behind the atomic write.
pub(crate) fn atomically_write(path: &Path, body: &[u8]) -> Result<(), RegistryError> {
    let io_err = |source| RegistryError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut file = AtomicWriteFile::open(path).map_err(io_err)?;
    file.write_all(body).map_err(io_err)?;
    file.commit().map_err(io_err)
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
