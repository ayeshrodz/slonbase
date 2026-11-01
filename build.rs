//! Embeds the admin dashboard (`ui/app`): text files are gzipped at build time, and
//! `$OUT_DIR/ui_files.rs` lists every file as `(path, bytes, gzipped, etag)`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const COMPRESSIBLE: &[&str] = &["html", "js", "css", "svg", "json", "txt", "map"];

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("can't read {}: {e}", dir.display()))
        .map(|e| e.expect("directory entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// A content hash for the file's ETag.
fn fnv1a(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// The page with its files under `./v/{version}/` (cached for good: a new build has another
/// version) and a preload of every module. `context.js` is generated per request, so it stays.
fn versioned_page(page: &str, version: &str, preloads: &str) -> String {
    let mut out = String::with_capacity(page.len() + preloads.len());
    for line in page.lines() {
        let line = if line.contains("./context.js") {
            line.to_string()
        } else {
            line.replace("\"./", &format!("\"./v/{version}/"))
        };
        if line.contains("type=\"module\"") {
            out.push_str(preloads);
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn main() {
    let dist = Path::new("ui/app");
    println!("cargo:rerun-if-changed=ui/app");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let embed_dir = out_dir.join("ui");
    let _ = fs::remove_dir_all(&embed_dir);

    let mut paths = Vec::new();
    files(dist, &mut paths);
    let relative_of = |path: &Path| {
        path.strip_prefix(dist)
            .expect("inside dist")
            .to_str()
            .expect("UTF-8 file name")
            .replace('\\', "/")
    };
    // the build's version: a hash of every file but the page, which names it
    let mut combined = Vec::new();
    for path in &paths {
        let relative = relative_of(path);
        if relative != "index.html" {
            combined.extend_from_slice(relative.as_bytes());
            combined.extend_from_slice(
                &fnv1a(&fs::read(path).expect("readable UI file")).to_le_bytes(),
            );
        }
    }
    let version = format!("{:016x}", fnv1a(&combined));
    // modules loaded on demand (`import("./x.js")`), which the page doesn't preload
    let mut on_demand = std::collections::HashSet::new();
    for path in &paths {
        let relative = relative_of(path);
        if !relative.ends_with(".js") {
            continue;
        }
        let code = fs::read_to_string(path).expect("UTF-8 module");
        let dir = Path::new(&relative).parent().unwrap_or(Path::new(""));
        for part in code.split("import(\"").skip(1) {
            let Some(target) = part.split('"').next().filter(|t| t.starts_with("./")) else {
                continue;
            };
            let resolved = dir.join(target.trim_start_matches("./"));
            on_demand.insert(resolved.to_str().expect("UTF-8 path").replace('\\', "/"));
        }
    }
    // every other module, fetched at once instead of as each import is found
    let preloads: String = paths
        .iter()
        .map(|p| relative_of(p))
        .filter(|r| r.ends_with(".js") && r != "boot.js" && !on_demand.contains(r))
        .map(|r| format!("    <link rel=\"modulepreload\" href=\"./v/{version}/{r}\" />\n"))
        .collect();

    let mut table = format!("pub static VERSION: &str = {version:?};\n");
    table.push_str("pub static FILES: &[(&str, &[u8], bool, &str)] = &[\n");
    for path in paths {
        let relative = relative_of(&path);
        let mut data = fs::read(&path).expect("readable UI file");
        if relative == "index.html" {
            data = versioned_page(
                &String::from_utf8(data).expect("UTF-8 page"),
                &version,
                &preloads,
            )
            .into_bytes();
        }
        let hash = fnv1a(&data);
        let gzip = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| COMPRESSIBLE.contains(&e));
        let target = embed_dir.join(if gzip {
            format!("{relative}.gz")
        } else {
            relative.clone()
        });
        fs::create_dir_all(target.parent().expect("has a parent")).expect("create dir");
        let bytes = if gzip {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            encoder.write_all(&data).expect("gzip");
            encoder.finish().expect("gzip")
        } else {
            data
        };
        fs::write(&target, bytes).expect("write embedded file");
        table.push_str(&format!(
            "    ({relative:?}, include_bytes!({:?}), {gzip}, \"\\\"{:016x}\\\"\"),\n",
            target.to_str().expect("UTF-8 path"),
            hash
        ));
    }
    table.push_str("];\n");
    fs::write(out_dir.join("ui_files.rs"), table).expect("write ui_files.rs");
}
