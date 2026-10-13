//! File storage. Keys look like `{collectionId}/{recordId}/{filename}`; only the local
//! filesystem is supported for now (S3-compatible storage is planned).

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;

pub struct Storage {
    root: PathBuf,
}

/// Each part of a key must be a plain name: no separators, no `..`, no hidden files.
fn valid_part(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 255
        && !part.starts_with('.')
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

impl Storage {
    pub fn local(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path(&self, key: &str) -> Result<PathBuf> {
        let mut path = self.root.clone();
        for part in key.split('/') {
            anyhow::ensure!(valid_part(part), "invalid storage key {key:?}");
            path.push(part);
        }
        debug_assert!(
            path.components()
                .all(|c| !matches!(c, Component::ParentDir))
        );
        Ok(path)
    }

    /// Writes a file atomically (temp file + rename).
    pub async fn put(&self, key: &str, data: &[u8]) -> Result<()> {
        let path = self.path(key)?;
        let dir = path.parent().context("key has no directory")?;
        tokio::fs::create_dir_all(dir)
            .await
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let tmp = dir.join(format!(".tmp-{}", crate::ids::random_id(12)));
        let result = async {
            let mut file = tokio::fs::File::create(&tmp).await?;
            file.write_all(data).await?;
            file.sync_all().await?;
            tokio::fs::rename(&tmp, &path).await?;
            anyhow::Ok(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
        }
        result.with_context(|| format!("failed to write {}", path.display()))
    }

    pub async fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        match tokio::fs::read(self.path(key)?).await {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Opens a file for streaming, with its size.
    pub async fn open(&self, key: &str) -> Result<Option<(tokio::fs::File, u64)>> {
        match tokio::fs::File::open(self.path(key)?).await {
            Ok(file) => {
                let size = file.metadata().await?.len();
                Ok(Some((file, size)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        match tokio::fs::remove_file(self.path(key)?).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// Deletes a directory key (e.g. a record's or a collection's files) recursively.
    pub async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        match tokio::fs::remove_dir_all(self.path(prefix)?).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_and_safety() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::local(dir.path());

        storage.put("c1/r1/a_x.txt", b"hello").await.unwrap();
        assert_eq!(
            storage.read("c1/r1/a_x.txt").await.unwrap().unwrap(),
            b"hello"
        );
        let (_, size) = storage.open("c1/r1/a_x.txt").await.unwrap().unwrap();
        assert_eq!(size, 5);
        assert!(storage.read("c1/r1/missing.txt").await.unwrap().is_none());

        storage.delete("c1/r1/a_x.txt").await.unwrap();
        storage.delete("c1/r1/a_x.txt").await.unwrap(); // idempotent
        storage
            .put("c1/r1/thumbs_b/100x100_b.png", b"x")
            .await
            .unwrap();
        storage.delete_prefix("c1/r1").await.unwrap();
        assert!(!dir.path().join("c1/r1").exists());

        for bad in [
            "../x",
            "c1/../../etc/passwd",
            "c1//x",
            "/abs",
            "c1/.hidden",
            "c1/a b",
            "c1/r1/x\\y",
        ] {
            assert!(storage.put(bad, b"x").await.is_err(), "{bad}");
            assert!(storage.read(bad).await.is_err(), "{bad}");
        }
    }
}
