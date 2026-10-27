//! Backup encryption at rest: a backup archive sealed with a passphrase (`backups.encryption_key`),
//! streamed so large backups never sit in memory.
//!
//! Format: a header (`MAGIC`, a format byte, the PBKDF2 iterations, a 16-byte salt and a 7-byte
//! nonce prefix), then the archive in 64 KiB chunks, each sealed with AES-256-GCM under a key
//! derived from the passphrase with PBKDF2-HMAC-SHA256. A chunk's nonce is the prefix, its
//! number and a last-chunk flag, and the header is authenticated with every chunk, so reordered,
//! dropped, truncated or altered chunks (and a changed header) fail to decrypt.

use std::io::{BufRead, BufReader, Read, Write};
use std::num::NonZeroU32;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};

pub const MAGIC: &[u8; 8] = b"SLONBENC";
const FORMAT: u8 = 1;
/// OWASP's recommendation for PBKDF2-HMAC-SHA256 (fewer in unit tests).
const ITERATIONS: u32 = if cfg!(test) { 1_000 } else { 600_000 };
const SALT: usize = 16;
const NONCE_PREFIX: usize = 7;
const HEADER: usize = MAGIC.len() + 1 + 4 + SALT + NONCE_PREFIX;
const CHUNK: usize = 64 << 10;
const TAG: usize = 16;
/// The shortest passphrase accepted.
pub const MIN_KEY_LENGTH: usize = 12;

/// Why a backup couldn't be decrypted.
#[derive(Debug)]
pub enum DecryptError {
    /// The passphrase is wrong, or the file was changed or cut short.
    Rejected,
    Other(anyhow::Error),
}

impl From<std::io::Error> for DecryptError {
    fn from(e: std::io::Error) -> Self {
        Self::Other(e.into())
    }
}

/// Whether a file starts like an encrypted backup.
pub fn is_encrypted(path: &Path) -> Result<bool> {
    let mut start = [0u8; MAGIC.len()];
    let mut file = std::fs::File::open(path)?;
    let mut read = 0;
    while read < start.len() {
        match file.read(&mut start[read..])? {
            0 => return Ok(false),
            n => read += n,
        }
    }
    Ok(&start == MAGIC)
}

fn key(passphrase: &str, salt: &[u8], iterations: u32) -> Result<LessSafeKey> {
    let iterations = NonZeroU32::new(iterations).context("invalid iterations")?;
    let mut key = [0u8; 32];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        salt,
        passphrase.as_bytes(),
        &mut key,
    );
    let key = UnboundKey::new(&AES_256_GCM, &key).map_err(|_| anyhow::anyhow!("invalid key"))?;
    Ok(LessSafeKey::new(key))
}

fn nonce(prefix: &[u8], counter: u32, last: bool) -> Nonce {
    let mut bytes = [0u8; 12];
    bytes[..NONCE_PREFIX].copy_from_slice(prefix);
    bytes[NONCE_PREFIX..NONCE_PREFIX + 4].copy_from_slice(&counter.to_be_bytes());
    bytes[11] = u8::from(last);
    Nonce::assume_unique_for_key(bytes)
}

/// Reads up to `buf.len()` bytes, fewer only at the end of the input.
fn read_full(input: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut read = 0;
    while read < buf.len() {
        match input.read(&mut buf[read..])? {
            0 => break,
            n => read += n,
        }
    }
    Ok(read)
}

/// Encrypts `src` into `dst` (blocking; the key derivation alone takes a moment).
pub fn encrypt_file(src: &Path, dst: &Path, passphrase: &str) -> Result<()> {
    let rng = SystemRandom::new();
    let mut header = Vec::with_capacity(HEADER);
    header.extend_from_slice(MAGIC);
    header.push(FORMAT);
    header.extend_from_slice(&ITERATIONS.to_be_bytes());
    let mut random = [0u8; SALT + NONCE_PREFIX];
    rng.fill(&mut random)
        .map_err(|_| anyhow::anyhow!("no randomness"))?;
    header.extend_from_slice(&random);
    let (salt, prefix) = random.split_at(SALT);
    let key = key(passphrase, salt, ITERATIONS)?;

    let mut input = BufReader::with_capacity(CHUNK * 2, std::fs::File::open(src)?);
    let mut output = std::io::BufWriter::new(std::fs::File::create(dst)?);
    output.write_all(&header)?;
    let mut counter: u32 = 0;
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let len = read_full(&mut input, &mut chunk)?;
        // the last chunk is the one with no more input after it (possibly empty)
        let last = len < CHUNK || input.fill_buf()?.is_empty();
        let mut sealed = chunk[..len].to_vec();
        key.seal_in_place_append_tag(
            nonce(prefix, counter, last),
            Aad::from(&header),
            &mut sealed,
        )
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;
        output.write_all(&sealed)?;
        if last {
            break;
        }
        counter = counter
            .checked_add(1)
            .context("the backup is too large to encrypt")?;
    }
    output.flush()?;
    output.into_inner()?.sync_all()?;
    Ok(())
}

/// Decrypts `src` (an encrypted backup) into `dst` (blocking). Nothing of a file that fails
/// to decrypt should be used: `dst` is incomplete then.
pub fn decrypt_file(src: &Path, dst: &Path, passphrase: &str) -> Result<(), DecryptError> {
    let mut input = BufReader::with_capacity((CHUNK + TAG) * 2, std::fs::File::open(src)?);
    let mut header = [0u8; HEADER];
    if read_full(&mut input, &mut header)? < HEADER || &header[..MAGIC.len()] != MAGIC {
        return Err(DecryptError::Other(anyhow::anyhow!(
            "the file isn't an encrypted backup"
        )));
    }
    let mut at = MAGIC.len();
    if header[at] != FORMAT {
        return Err(DecryptError::Other(anyhow::anyhow!(
            "the backup was encrypted by a newer slonbase"
        )));
    }
    at += 1;
    let iterations = u32::from_be_bytes(header[at..at + 4].try_into().expect("4 bytes"));
    // a damaged header can't make the key derivation take forever
    if !(1..=10_000_000).contains(&iterations) {
        return Err(DecryptError::Rejected);
    }
    at += 4;
    let salt = &header[at..at + SALT];
    let prefix = &header[at + SALT..at + SALT + NONCE_PREFIX];
    let key = key(passphrase, salt, iterations).map_err(DecryptError::Other)?;

    let mut output = std::io::BufWriter::new(std::fs::File::create(dst)?);
    let mut counter: u32 = 0;
    let mut sealed = vec![0u8; CHUNK + TAG];
    loop {
        let len = read_full(&mut input, &mut sealed)?;
        let last = len < CHUNK + TAG || input.fill_buf()?.is_empty();
        if len < TAG {
            return Err(DecryptError::Rejected);
        }
        let plain = key
            .open_in_place(
                nonce(prefix, counter, last),
                Aad::from(&header),
                &mut sealed[..len],
            )
            .map_err(|_| DecryptError::Rejected)?;
        output.write_all(plain)?;
        if last {
            break;
        }
        counter = counter.checked_add(1).ok_or(DecryptError::Rejected)?;
    }
    output.flush()?;
    Ok(())
}

/// Checks a configured passphrase.
pub fn check_key(passphrase: &str) -> Result<()> {
    if passphrase.chars().count() < MIN_KEY_LENGTH {
        bail!("the backup encryption key must be at least {MIN_KEY_LENGTH} characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(size: usize) {
        let dir = tempfile::tempdir().unwrap();
        let (plain, sealed, opened) = (
            dir.path().join("plain"),
            dir.path().join("sealed"),
            dir.path().join("opened"),
        );
        let data: Vec<u8> = (0..size).map(|i| (i * 7 % 256) as u8).collect();
        std::fs::write(&plain, &data).unwrap();
        encrypt_file(&plain, &sealed, "correct horse battery").unwrap();
        assert!(is_encrypted(&sealed).unwrap());
        assert!(!is_encrypted(&plain).unwrap() || size == 0);
        let raw = std::fs::read(&sealed).unwrap();
        // a full last chunk is still the last one
        let chunks = size.div_ceil(CHUNK).max(1);
        assert_eq!(raw.len(), HEADER + size + chunks * TAG, "size {size}");
        decrypt_file(&sealed, &opened, "correct horse battery").unwrap();
        assert_eq!(std::fs::read(&opened).unwrap(), data, "size {size}");
    }

    #[test]
    fn roundtrips() {
        for size in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK, 3 * CHUNK + 17] {
            roundtrip(size);
        }
    }

    #[test]
    fn rejects_wrong_keys_and_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let (plain, sealed, out) = (
            dir.path().join("plain"),
            dir.path().join("sealed"),
            dir.path().join("out"),
        );
        let data: Vec<u8> = (0..(2 * CHUNK + 100)).map(|i| (i % 256) as u8).collect();
        std::fs::write(&plain, &data).unwrap();
        encrypt_file(&plain, &sealed, "correct horse battery").unwrap();
        assert!(matches!(
            decrypt_file(&sealed, &out, "wrong horse battery"),
            Err(DecryptError::Rejected)
        ));
        let raw = std::fs::read(&sealed).unwrap();
        let chunk = CHUNK + TAG;
        let mut flipped = raw.clone();
        flipped[HEADER + 10] ^= 1;
        let mut header = raw.clone();
        header[HEADER - 1] ^= 1;
        let truncated = raw[..HEADER + 2 * chunk].to_vec(); // the last chunk dropped
        let mut swapped = raw[..HEADER].to_vec();
        swapped.extend_from_slice(&raw[HEADER + chunk..HEADER + 2 * chunk]);
        swapped.extend_from_slice(&raw[HEADER..HEADER + chunk]);
        swapped.extend_from_slice(&raw[HEADER + 2 * chunk..]);
        for (what, bytes) in [
            ("flipped", flipped),
            ("header", header),
            ("truncated", truncated),
            ("swapped", swapped),
        ] {
            std::fs::write(&sealed, bytes).unwrap();
            assert!(
                matches!(
                    decrypt_file(&sealed, &out, "correct horse battery"),
                    Err(DecryptError::Rejected)
                ),
                "{what}"
            );
        }
        std::fs::write(&sealed, b"PK\x03\x04 a zip").unwrap();
        assert!(!is_encrypted(&sealed).unwrap());
        assert!(matches!(
            decrypt_file(&sealed, &out, "correct horse battery"),
            Err(DecryptError::Other(_))
        ));
        assert!(check_key("short").is_err());
        assert!(check_key("long enough key").is_ok());
    }
}
