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
/// is. A path that cannot be written without the prefix keeps it: one that is not valid Unicode,
/// or that has a name Windows would change without it (a trailing dot or space).
#[cfg_attr(not(windows), allow(dead_code))]
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path;
    };
    let plain = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        Some(format!(r"\\{rest}"))
    } else {
        text.strip_prefix(r"\\?\")
            .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
            .map(str::to_owned)
    };
    match plain {
        Some(plain)
            if !plain
                .split('\\')
                .any(|name| name.ends_with(['.', ' ']) && name != "." && name != "..") =>
        {
            PathBuf::from(plain)
        }
        _ => path,
    }
}

/// Whether `path` is `base` or inside it, comparing the two without their `\\?\` prefix: a path
/// that keeps the prefix (see [`canonicalize`]) is still inside a base that lost it.
pub fn is_within(path: &Path, base: &Path) -> bool {
    comparable(path).starts_with(comparable(base))
}

fn comparable(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path.to_path_buf(),
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
        // a name Windows would change without the prefix keeps it
        assert_eq!(
            strip(r"\\?\C:\data\report."),
            PathBuf::from(r"\\?\C:\data\report.")
        );
    }

    #[test]
    fn a_path_is_resolved_like_canonicalize_does() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = canonicalize(dir.path()).unwrap();
        assert!(resolved.is_absolute());
        assert!(canonicalize(dir.path().join("missing")).is_err());
    }

    #[test]
    fn a_path_is_inside_a_base_by_whole_components() {
        assert!(is_within(
            Path::new("/srv/files/a"),
            Path::new("/srv/files")
        ));
        assert!(!is_within(
            Path::new("/srv/other/a"),
            Path::new("/srv/files")
        ));
        assert!(!is_within(
            Path::new("/srv/files2/a"),
            Path::new("/srv/files")
        ));
    }

    #[cfg(windows)]
    #[test]
    fn a_path_that_kept_the_prefix_is_inside_a_base_that_lost_it() {
        assert!(is_within(
            Path::new(r"\\?\C:\data\report."),
            Path::new(r"C:\data")
        ));
        assert!(is_within(
            Path::new(r"C:\data\x"),
            Path::new(r"\\?\C:\data")
        ));
        assert!(!is_within(
            Path::new(r"\\?\C:\other\x"),
            Path::new(r"C:\data")
        ));
    }
}
