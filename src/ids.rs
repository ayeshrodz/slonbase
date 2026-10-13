//! Random identifiers and secrets.

const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Default record id length (matches PocketBase's 15 character ids).
pub const DEFAULT_ID_LENGTH: usize = 15;

/// Returns a random `[a-z0-9]` string of the given length from a CSPRNG.
pub fn random_id(len: usize) -> String {
    // rejection sampling keeps the distribution uniform:
    // 252 is the largest multiple of 36 that fits in a byte
    const LIMIT: u8 = (256 / ID_ALPHABET.len() * ID_ALPHABET.len()) as u8;

    let mut out = String::with_capacity(len);
    let mut buf = [0u8; 64];
    while out.len() < len {
        rand::fill(&mut buf[..]);
        for &b in &buf {
            if b < LIMIT {
                out.push(ID_ALPHABET[(b as usize) % ID_ALPHABET.len()] as char);
                if out.len() == len {
                    break;
                }
            }
        }
    }
    out
}

/// Returns a hex encoded secret with `bytes` bytes of entropy.
pub fn random_secret(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::fill(&mut buf[..]);
    hex::encode(buf)
}

/// Compares two strings in constant time (relative to their content).
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_expected_shape() {
        for len in [1, 15, 100] {
            let id = random_id(len);
            assert_eq!(id.len(), len);
            assert!(id.bytes().all(|b| ID_ALPHABET.contains(&b)));
        }
        assert_ne!(random_id(15), random_id(15));
    }

    #[test]
    fn secrets_are_hex() {
        let s = random_secret(32);
        assert_eq!(s.len(), 64);
        assert!(s.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(constant_time_eq("", ""));
    }
}
