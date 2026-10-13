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

/// Generates a value for an `autogeneratePattern`. Only the `[charset]{n}` form is
/// supported (e.g. `[a-z0-9]{15}`), where the charset may contain ranges.
/// Returns `None` for unsupported patterns.
pub fn generate_from_pattern(pattern: &str) -> Option<String> {
    let (charset, len) = parse_pattern(pattern)?;
    let limit = (256 / charset.len() * charset.len()) as u16;
    let mut out = String::with_capacity(len);
    let mut buf = [0u8; 64];
    while out.chars().count() < len {
        rand::fill(&mut buf[..]);
        for &b in &buf {
            if u16::from(b) < limit {
                out.push(charset[(b as usize) % charset.len()]);
                if out.chars().count() == len {
                    break;
                }
            }
        }
    }
    Some(out)
}

/// Validates an `autogeneratePattern` (see [`generate_from_pattern`]).
pub fn is_supported_pattern(pattern: &str) -> bool {
    parse_pattern(pattern).is_some()
}

fn parse_pattern(pattern: &str) -> Option<(Vec<char>, usize)> {
    let rest = pattern.strip_prefix('[')?;
    let (class, rest) = rest.split_once(']')?;
    let count = rest.strip_prefix('{')?.strip_suffix('}')?;
    let len: usize = count.parse().ok().filter(|n| (1..=255).contains(n))?;

    let chars: Vec<char> = class.chars().collect();
    let mut charset = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if i + 2 < chars.len() && chars[i + 1] == '-' {
            let (from, to) = (chars[i], chars[i + 2]);
            if from > to || !from.is_ascii() || !to.is_ascii() {
                return None;
            }
            charset.extend(from..=to);
            i += 3;
        } else {
            charset.push(chars[i]);
            i += 1;
        }
    }
    charset.sort_unstable();
    charset.dedup();
    if charset.is_empty() || charset.len() > 256 || charset.iter().any(|c| !c.is_ascii_graphic()) {
        return None;
    }
    Some((charset, len))
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
    fn patterns() {
        let v = generate_from_pattern("[a-z0-9]{15}").unwrap();
        assert_eq!(v.len(), 15);
        assert!(
            v.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        );

        let v = generate_from_pattern("[A-F_]{40}").unwrap();
        assert!(v.chars().all(|c| ('A'..='F').contains(&c) || c == '_'));

        for bad in [
            "",
            "abc",
            "[a-z]",
            "[a-z]{0}",
            "[a-z]{256}",
            "[z-a]{3}",
            "[]{3}",
            "[a-z]{3}x",
            "[a b]{2}",
        ] {
            assert!(!is_supported_pattern(bad), "{bad}");
        }
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
