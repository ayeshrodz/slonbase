//! File storage. Keys look like `{collectionId}/{recordId}/{filename}` and are stored on
//! the local filesystem or in an S3 bucket (chosen in the settings).

use std::path::{Component, Path, PathBuf};
use std::pin::Pin;

use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::Stream;
use tokio::io::AsyncWriteExt;

use crate::s3;

pub type ByteStream = Pin<Box<dyn Stream<Item = std::io::Result<Bytes>> + Send>>;

/// A stored file opened for reading.
pub struct Object {
    pub size: u64,
    pub stream: ByteStream,
}

pub enum Storage {
    Local(Local),
    S3(Box<s3::Client>),
    /// Misconfigured storage: every operation fails with this message (rather than
    /// silently writing somewhere else).
    Unavailable(String),
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

fn check_key(key: &str) -> Result<()> {
    anyhow::ensure!(
        key.split('/').all(valid_part),
        "invalid storage key {key:?}"
    );
    Ok(())
}

impl Storage {
    pub fn local(root: impl Into<PathBuf>) -> Self {
        Self::Local(Local { root: root.into() })
    }

    pub fn s3(config: s3::Config) -> Self {
        match s3::Client::new(config) {
            Ok(client) => Self::S3(Box::new(client)),
            Err(err) => Self::Unavailable(format!("{err:#}")),
        }
    }

    fn unavailable(message: &str) -> anyhow::Error {
        anyhow::anyhow!("the file storage is unavailable: {message}")
    }

    /// A short description for logs.
    pub fn describe(&self) -> String {
        match self {
            Self::Local(local) => format!("local directory {}", local.root.display()),
            Self::S3(client) => format!(
                "S3 bucket {} at {}",
                client.config().bucket,
                client.config().endpoint
            ),
            Self::Unavailable(message) => format!("unavailable ({message})"),
        }
    }

    /// Writes a file (atomically on the local filesystem).
    pub async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        check_key(key)?;
        match self {
            Self::Local(local) => local.put(key, &data).await,
            Self::S3(client) => client.put(key, data).await,
            Self::Unavailable(message) => Err(Self::unavailable(message)),
        }
    }

    pub async fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        check_key(key)?;
        match self {
            Self::Local(local) => match tokio::fs::read(local.path(key)).await {
                Ok(data) => Ok(Some(data)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            },
            _ => {
                use futures_util::TryStreamExt;
                let Some(object) = self.open(key).await? else {
                    return Ok(None);
                };
                let mut data = Vec::with_capacity(object.size as usize);
                let mut stream = object.stream;
                while let Some(chunk) = stream.try_next().await? {
                    data.extend_from_slice(&chunk);
                }
                Ok(Some(data))
            }
        }
    }

    /// Opens a file for streaming, with its size.
    pub async fn open(&self, key: &str) -> Result<Option<Object>> {
        check_key(key)?;
        match self {
            Self::Local(local) => match tokio::fs::File::open(local.path(key)).await {
                Ok(file) => {
                    let size = file.metadata().await?.len();
                    Ok(Some(Object {
                        size,
                        stream: Box::pin(file_chunks(file)),
                    }))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            },
            Self::S3(client) => client.open(key).await,
            Self::Unavailable(message) => Err(Self::unavailable(message)),
        }
    }

    /// The size of a file, or `None` when it doesn't exist.
    pub async fn size(&self, key: &str) -> Result<Option<u64>> {
        check_key(key)?;
        match self {
            Self::Local(local) => match tokio::fs::metadata(local.path(key)).await {
                Ok(meta) => Ok(Some(meta.len())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            },
            Self::S3(client) => client.size(key).await,
            Self::Unavailable(message) => Err(Self::unavailable(message)),
        }
    }

    /// Opens bytes `start..=end` of a file (`size` is the length of that range).
    pub async fn open_range(&self, key: &str, start: u64, end: u64) -> Result<Option<Object>> {
        check_key(key)?;
        anyhow::ensure!(start <= end, "invalid range");
        let len = end - start + 1;
        match self {
            Self::Local(local) => match tokio::fs::File::open(local.path(key)).await {
                Ok(mut file) => {
                    use tokio::io::{AsyncReadExt, AsyncSeekExt};
                    file.seek(std::io::SeekFrom::Start(start)).await?;
                    Ok(Some(Object {
                        size: len,
                        stream: Box::pin(file_chunks(file.take(len))),
                    }))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            },
            Self::S3(client) => client.open_range(key, start, end).await,
            Self::Unavailable(message) => Err(Self::unavailable(message)),
        }
    }

    pub async fn exists(&self, key: &str) -> Result<bool> {
        check_key(key)?;
        match self {
            Self::Local(local) => Ok(tokio::fs::try_exists(local.path(key)).await?),
            Self::S3(client) => client.exists(key).await,
            Self::Unavailable(message) => Err(Self::unavailable(message)),
        }
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        check_key(key)?;
        match self {
            Self::Local(local) => match tokio::fs::remove_file(local.path(key)).await {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            },
            Self::S3(client) => client.delete(key).await,
            Self::Unavailable(message) => Err(Self::unavailable(message)),
        }
    }

    /// Deletes every file under a directory key (e.g. a record's or a collection's files).
    pub async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        check_key(prefix)?;
        match self {
            Self::Local(local) => match tokio::fs::remove_dir_all(local.path(prefix)).await {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            },
            Self::S3(client) => {
                let keys = client.list(&format!("{prefix}/")).await?;
                client.delete_many(&keys).await
            }
            Self::Unavailable(message) => Err(Self::unavailable(message)),
        }
    }

    /// Stores a file from disk (moved when the storage is local, streamed to S3).
    pub async fn put_file(&self, key: &str, path: &Path) -> Result<()> {
        check_key(key)?;
        match self {
            Self::Local(local) => {
                let target = local.path(key);
                if let Some(dir) = target.parent() {
                    tokio::fs::create_dir_all(dir).await?;
                }
                if tokio::fs::rename(path, &target).await.is_err() {
                    // another file system: copy, then remove the source
                    let tmp = target.with_extension("tmp-copy");
                    tokio::fs::copy(path, &tmp).await?;
                    tokio::fs::rename(&tmp, &target).await?;
                    let _ = tokio::fs::remove_file(path).await;
                }
                Ok(())
            }
            Self::S3(client) => client.put_file(key, path).await,
            Self::Unavailable(message) => Err(Self::unavailable(message)),
        }
    }

    /// The files directly under the root (local) or in the bucket's root (S3), with their
    /// size and modification time (`YYYY-MM-DD HH:MM:SS.sssZ`).
    pub async fn list_root(&self) -> Result<Vec<(String, u64, String)>> {
        match self {
            Self::Local(local) => {
                let mut out = Vec::new();
                let mut entries = match tokio::fs::read_dir(&local.root).await {
                    Ok(entries) => entries,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
                    Err(e) => return Err(e.into()),
                };
                while let Some(entry) = entries.next_entry().await? {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let meta = entry.metadata().await?;
                    if !meta.is_file() || !valid_part(&name) {
                        continue;
                    }
                    let modified: chrono::DateTime<chrono::Utc> = meta.modified()?.into();
                    out.push((name, meta.len(), crate::datetime::format(&modified)));
                }
                Ok(out)
            }
            Self::S3(client) => Ok(client
                .list_objects("")
                .await?
                .into_iter()
                .filter(|o| !o.key.contains('/') && valid_part(&o.key))
                .map(|o| {
                    let modified = crate::datetime::parse(&o.modified)
                        .map(|d| crate::datetime::format(&d))
                        .unwrap_or(o.modified);
                    (o.key, o.size, modified)
                })
                .collect()),
            Self::Unavailable(message) => Err(Self::unavailable(message)),
        }
    }

    /// Checks that files can be written, read and deleted (`/api/settings/test/s3`).
    pub async fn check(&self) -> Result<()> {
        let key = format!("_slonbase_test_{}.txt", crate::ids::random_id(12));
        let content = b"slonbase storage test";
        self.put(&key, Bytes::from_static(content))
            .await
            .context("write failed")?;
        let read = self.read(&key).await.context("read failed");
        let deleted = self.delete(&key).await.context("delete failed");
        anyhow::ensure!(
            read?.as_deref() == Some(&content[..]),
            "the written file read back differently"
        );
        deleted
    }
}

pub struct Local {
    root: PathBuf,
}

impl Local {
    fn path(&self, key: &str) -> PathBuf {
        let mut path = self.root.clone();
        path.extend(key.split('/'));
        debug_assert!(
            path.components()
                .all(|c| !matches!(c, Component::ParentDir))
        );
        path
    }

    async fn put(&self, key: &str, data: &[u8]) -> Result<()> {
        let path = self.path(key);
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

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Streams a file (or any reader) in 64 KiB chunks.
pub(crate) fn file_chunks<R: tokio::io::AsyncRead + Unpin + Send>(
    file: R,
) -> impl Stream<Item = std::io::Result<Bytes>> + Send {
    futures_util::stream::unfold(file, |mut file| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 64 * 1024];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(Bytes::from(buf)), file))
            }
            Err(e) => Some((Err(e), file)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip_and_safety() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::local(dir.path());

        storage
            .put("c1/r1/a_x.txt", Bytes::from_static(b"hello"))
            .await
            .unwrap();
        assert_eq!(
            storage.read("c1/r1/a_x.txt").await.unwrap().unwrap(),
            b"hello"
        );
        let object = storage.open("c1/r1/a_x.txt").await.unwrap().unwrap();
        assert_eq!(object.size, 5);
        assert!(storage.exists("c1/r1/a_x.txt").await.unwrap());
        storage.check().await.unwrap();
        assert!(storage.read("c1/r1/missing.txt").await.unwrap().is_none());

        storage.delete("c1/r1/a_x.txt").await.unwrap();
        storage.delete("c1/r1/a_x.txt").await.unwrap(); // idempotent
        storage
            .put("c1/r1/thumbs_b/100x100_b.png", Bytes::from_static(b"x"))
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
            assert!(
                storage.put(bad, Bytes::from_static(b"x")).await.is_err(),
                "{bad}"
            );
            assert!(storage.read(bad).await.is_err(), "{bad}");
        }
    }
}
