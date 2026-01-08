fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() != "windows" {
        return;
    }

    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};

    println!("cargo:rerun-if-env-changed=QPDF_PATH");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let tool_path = manifest_dir.join("tools").join("qpdf.exe");

    let qpdf_path = match env::var("QPDF_PATH").ok().filter(|s| !s.is_empty()) {
        Some(path) => PathBuf::from(path),
        None if tool_path.exists() => tool_path,
        None => {
            println!("cargo:warning=Windows build: qpdf.exe not found in tools/");
            return;
        }
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let target_dir = out_dir
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| out_dir.clone());

    let dest_path = target_dir.join("qpdf.exe");
    if let Err(err) = fs::copy(&qpdf_path, &dest_path) {
        println!("cargo:warning=Failed to copy qpdf.exe: {err}");
    }

    if let Some(parent) = qpdf_path.parent() {
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if ext.eq_ignore_ascii_case("dll") {
                        if let Some(file_name) = path.file_name() {
                            let dest_dll = target_dir.join(file_name);
                            let _ = fs::copy(&path, &dest_dll);
                        }
                    }
                }
            }
        }
    }
}
