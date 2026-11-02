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

/// Generates a value for an `autogeneratePattern`: a random string the regular expression
/// matches, as PocketBase does. `[charset]{n}` patterns (e.g. `[a-z0-9]{15}`) take a fast path.
/// Returns `None` for patterns [`generate`] refuses.
pub fn generate_from_pattern(pattern: &str) -> Option<String> {
    match parse_pattern(pattern) {
        Some((charset, len)) => Some(from_charset(&charset, len)),
        None => generate(pattern).ok(),
    }
}

fn from_charset(charset: &[char], len: usize) -> String {
    let limit = (256 / charset.len() * charset.len()) as u16;
    let mut out = String::with_capacity(len);
    let mut count = 0;
    let mut buf = [0u8; 64];
    while count < len {
        rand::fill(&mut buf[..]);
        for &b in &buf {
            if u16::from(b) < limit {
                out.push(charset[(b as usize) % charset.len()]);
                count += 1;
                if count == len {
                    break;
                }
            }
        }
    }
    out
}

/// Validates an `autogeneratePattern` (see [`generate`]).
pub fn is_supported_pattern(pattern: &str) -> bool {
    check_pattern(pattern).is_ok()
}

/// Why an `autogeneratePattern` can't be used, if it can't.
pub fn check_pattern(pattern: &str) -> Result<(), String> {
    if parse_pattern(pattern).is_some() {
        return Ok(());
    }
    generate(pattern).map(drop)
}

/// The most a generated value may be, in bytes.
const MAX_GENERATED: usize = 10_000;
/// The most pattern nodes one value may visit, so `(a{0}){1000000000}` can't spin.
const MAX_STEPS: u32 = 100_000;
/// Unbounded repetitions (`*`, `+`, `{n,}`) repeat up to this many times, as in PocketBase.
const DEFAULT_MAX_REPEAT: u32 = 6;
/// Classes larger than this (`.`, `[^...]`, `\W`) are narrowed to printable ASCII.
const LARGE_CLASS: u32 = 256;

/// A random string the regular expression matches (PocketBase's `RandomStringByRegex`).
///
/// Character classes, literals, groups, alternations and repetitions are supported; anchors
/// and word boundaries aren't. `\d`, `\w` and `\s` are ASCII, like Go's. `.` gives letters
/// and digits, and other large classes (`[^...]`, `\W`) printable ASCII characters from the
/// class, so values stay readable.
pub fn generate(pattern: &str) -> Result<String, String> {
    // ASCII semantics first, as Go's regexp; patterns with non-ASCII classes need Unicode
    let ascii = regex_syntax::ParserBuilder::new()
        .unicode(false)
        .utf8(false)
        .nest_limit(50)
        .build()
        .parse(pattern);
    let hir = match ascii {
        Ok(hir) => hir,
        Err(_) => regex_syntax::ParserBuilder::new()
            .nest_limit(50)
            .build()
            .parse(pattern)
            .map_err(|e| {
                // the last "error:" line is the reason; the rest points into the pattern
                let text = e.to_string();
                text.lines()
                    .rev()
                    .find(|l| l.starts_with("error:"))
                    .map(|l| l.trim_start_matches("error:").trim().to_string())
                    .unwrap_or(text)
            })?,
    };
    let mut out = Vec::new();
    let mut steps = MAX_STEPS;
    write_random(&hir, &mut out, &mut steps)?;
    String::from_utf8(out).map_err(|_| "The pattern can produce invalid text.".to_string())
}

fn write_random(
    hir: &regex_syntax::hir::Hir,
    out: &mut Vec<u8>,
    steps: &mut u32,
) -> Result<(), String> {
    use regex_syntax::hir::{Class, HirKind};
    if *steps == 0 {
        return Err("The pattern is too complex.".into());
    }
    *steps -= 1;
    match hir.kind() {
        HirKind::Empty => {}
        HirKind::Literal(lit) => out.extend_from_slice(&lit.0),
        HirKind::Class(Class::Unicode(class)) => {
            let ranges: Vec<(u32, u32)> = class
                .ranges()
                .iter()
                .map(|r| (r.start() as u32, r.end() as u32))
                .collect();
            let c = random_char(&ranges, false)?;
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
        HirKind::Class(Class::Bytes(class)) => {
            let ranges: Vec<(u32, u32)> = class
                .ranges()
                .iter()
                .map(|r| (u32::from(r.start()), u32::from(r.end())))
                .collect();
            let c = random_char(&ranges, true)?;
            if !c.is_ascii() {
                return Err("The pattern can produce invalid text.".into());
            }
            out.push(c as u8);
        }
        HirKind::Look(_) => {
            return Err("Anchors and word boundaries aren't supported.".into());
        }
        HirKind::Repetition(rep) => {
            let max = rep.max.unwrap_or(DEFAULT_MAX_REPEAT).max(rep.min);
            let n = if max == rep.min {
                rep.min
            } else {
                rand::random_range(rep.min..=max)
            };
            for _ in 0..n {
                write_random(&rep.sub, out, steps)?;
                if out.len() > MAX_GENERATED {
                    break;
                }
            }
        }
        HirKind::Capture(cap) => write_random(&cap.sub, out, steps)?,
        HirKind::Concat(subs) => {
            for sub in subs {
                write_random(sub, out, steps)?;
            }
        }
        HirKind::Alternation(subs) => {
            write_random(&subs[rand::random_range(0..subs.len())], out, steps)?;
        }
    }
    if out.len() > MAX_GENERATED {
        return Err(format!(
            "The pattern produces values longer than {MAX_GENERATED} bytes."
        ));
    }
    Ok(())
}

/// A random character from the ranges (inclusive code points). In byte mode (ASCII patterns),
/// a class reaching past ASCII is a negated or `.` one: it's narrowed like a large class.
fn random_char(ranges: &[(u32, u32)], bytes: bool) -> Result<char, String> {
    const ALNUM: &[(u32, u32)] = &[(0x30, 0x39), (0x41, 0x5A), (0x61, 0x7A)];
    let size = |ranges: &[(u32, u32)]| ranges.iter().map(|(a, b)| b - a + 1).sum::<u32>();
    let narrowed: Vec<(u32, u32)>;
    let mut ranges = ranges;
    let past_ascii = bytes && ranges.last().is_some_and(|&(_, b)| b > 0x7F);
    if past_ascii || size(ranges) > LARGE_CLASS {
        // `.` (anything but a newline, or anything with `s`) gives letters and digits
        let newline = ranges.iter().any(|&(a, b)| a <= 0x0A && 0x0A <= b);
        let all_but_newline = !newline && ranges.first() == Some(&(0, 0x09));
        let everything = ranges.first().is_some_and(|&(a, _)| a == 0)
            && ranges
                .windows(2)
                .all(|w| w[0].1 + 1 == w[1].0 || (w[0].1 == 0xD7FF && w[1].0 == 0xE000));
        if all_but_newline || everything {
            ranges = ALNUM;
        } else {
            narrowed = ranges
                .iter()
                .filter_map(|&(a, b)| {
                    let (a, b) = (a.max(0x21), b.min(0x7E));
                    (a <= b).then_some((a, b))
                })
                .collect();
            if !narrowed.is_empty() || bytes {
                ranges = &narrowed;
            }
        }
    }
    let total = size(ranges);
    if total == 0 {
        return Err("The pattern has an empty character class.".into());
    }
    loop {
        let mut pick = rand::random_range(0..total);
        for &(a, b) in ranges {
            let len = b - a + 1;
            if pick < len {
                // (a Unicode class never includes surrogates)
                if let Some(c) = char::from_u32(a + pick) {
                    return Ok(c);
                }
                break;
            }
            pick -= len;
        }
    }
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

        // the fast path keeps its exact shape, Unicode included
        let v = generate_from_pattern("[äöü]{8}").unwrap();
        assert!(v.chars().count() == 8 && v.chars().all(|c| "äöü".contains(c)));
    }

    /// Generates `n` values and checks each against the pattern itself.
    fn matches_its_pattern(pattern: &str, n: usize) -> Vec<String> {
        let re = regex::Regex::new(&format!("^(?:{pattern})$")).unwrap();
        (0..n)
            .map(|_| {
                let v = generate(pattern).unwrap_or_else(|e| panic!("{pattern}: {e}"));
                assert!(re.is_match(&v), "{pattern} gave {v:?}");
                v
            })
            .collect()
    }

    #[test]
    fn any_regular_expression() {
        for pattern in [
            "INV-[0-9]{6}",
            "[a-z]{3}-[0-9]{3}",
            "(foo|bar|baz)_[a-f0-9]{8}",
            r"\d{4}-\d{2}",
            r"\w{10}",
            "a+b*c?",
            "x{2,5}",
            "[A-Z]{2,}",
            "(ab){3}",
            "(?i)abc",
            "[^0-9]{5}",
            r"\W{4}",
            ".{12}",
            "(?s).{12}",
            "[а-я]{6}",
            "é{3}",
            "\\.",
            "[[:alpha:]]{5}",
            "",
        ] {
            matches_its_pattern(pattern, 50);
        }
        // \d and \w are ASCII, as in Go
        for v in matches_its_pattern(r"\d{20}", 20) {
            assert!(v.bytes().all(|b| b.is_ascii_digit()), "{v}");
        }
        for v in matches_its_pattern(r"\w{20}", 20) {
            assert!(
                v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                "{v}"
            );
        }
        // `.` gives letters and digits; other large classes printable ASCII
        for v in matches_its_pattern(".{30}", 20) {
            assert!(v.bytes().all(|b| b.is_ascii_alphanumeric()), "{v}");
        }
        for v in matches_its_pattern("[^a-z]{30}", 20) {
            assert!(v.bytes().all(|b| b.is_ascii_graphic()), "{v}");
        }
        // unbounded repetitions stop at 6 (or the minimum)
        for v in matches_its_pattern("a*", 50) {
            assert!(v.len() <= 6);
        }
        assert!(
            matches_its_pattern("a{9,}", 10)
                .iter()
                .all(|v| v.len() == 9)
        );
        // every branch and length shows up
        let seen: std::collections::HashSet<String> = matches_its_pattern("(foo|bar)x?", 200)
            .into_iter()
            .collect();
        assert_eq!(seen.len(), 4, "{seen:?}");
    }

    #[test]
    fn refused_patterns() {
        for bad in [
            "[z-a]{3}",
            "[]{3}",
            "(abc",
            "a{3",
            "^abc$",
            r"\bword",
            "a{5000}{5000}",
            "(a|b{0}){100000}",
            "[a-z]{10001}",
            "[^\\x00-\\x{10FFFF}]",
        ] {
            assert!(!is_supported_pattern(bad), "{bad}");
            assert!(generate_from_pattern(bad).is_none(), "{bad}");
        }
        assert!(
            check_pattern("(abc")
                .unwrap_err()
                .contains("unclosed group")
        );
        // patterns that produce little but repeat a lot finish (or are refused) at once
        let started = std::time::Instant::now();
        for pattern in ["(a{0}){1000000000}", "(){1000000000}", "((x?){1000}){1000}"] {
            let _ = generate(pattern);
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
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
