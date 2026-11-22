//! File system helpers that behave the same on every platform.

use std::io;
use std::path::{Path, PathBuf};

/// [`std::fs::canonicalize`] without the `\\?\` prefix Windows puts on its result: a path with it
/// accepts no `/` as a separator, so a name joined to it with one, or a comparison with a path
/// written the usual way, fails. On other platforms it is `canonicalize` itself.
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let resolved = std::fs::canonicalize(path)?;
    #[cfg(windows)]
    return Ok(without_verbatim_prefix(resolved));
    #[cfg(not(windows))]
    Ok(resolved)
}

/// `\\?\C:\dir` as `C:\dir` and `\\?\UNC\server\share` as `\\server\share`; any other path as it
/// is (one that cannot be written without the prefix keeps it).
#[cfg_attr(not(windows), allow(dead_code))]
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy().into_owned();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verbatim_prefix_is_removed_where_the_path_can_be_written_without_it() {
        let strip = |p: &str| without_verbatim_prefix(PathBuf::from(p));
        assert_eq!(
            strip(r"\\?\C:\Users\runner\hooks"),
            PathBuf::from(r"C:\Users\runner\hooks")
        );
        assert_eq!(
            strip(r"\\?\UNC\server\share\x"),
            PathBuf::from(r"\\server\share\x")
        );
        // not a drive letter: kept as it is
        assert_eq!(
            strip(r"\\?\Volume{1234}\x"),
            PathBuf::from(r"\\?\Volume{1234}\x")
        );
        assert_eq!(strip("/usr/lib"), PathBuf::from("/usr/lib"));
    }

    #[test]
    fn a_path_is_resolved_like_canonicalize_does() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = canonicalize(dir.path()).unwrap();
        assert!(resolved.is_absolute());
        assert!(canonicalize(dir.path().join("missing")).is_err());
    }
}
