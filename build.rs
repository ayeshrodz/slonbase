//! Embeds the admin dashboard (`ui/dist`): text files are gzipped at build time, and
//! `$OUT_DIR/ui_files.rs` lists every file as `(path, bytes, gzipped)`.

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

fn main() {
    let dist = Path::new("ui/dist");
    println!("cargo:rerun-if-changed=ui/dist");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let embed_dir = out_dir.join("ui");
    let _ = fs::remove_dir_all(&embed_dir);

    let mut paths = Vec::new();
    files(dist, &mut paths);
    let mut table = String::from("pub static FILES: &[(&str, &[u8], bool)] = &[\n");
    for path in paths {
        let relative = path
            .strip_prefix(dist)
            .expect("inside dist")
            .to_str()
            .expect("UTF-8 file name")
            .replace('\\', "/");
        let data = fs::read(&path).expect("readable UI file");
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
            "    ({relative:?}, include_bytes!({:?}), {gzip}),\n",
            target.to_str().expect("UTF-8 path")
        ));
    }
    table.push_str("];\n");
    fs::write(out_dir.join("ui_files.rs"), table).expect("write ui_files.rs");
}
