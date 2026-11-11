//! Android: Opus compiled from its own source in `opus/` (xiph/opus, BSD), as a static library the
//! engine links, since a phone has no libopus to load. The workflow puts the source there
//! (`packaging/android/fetch_opus.sh`). Other targets load the system's libopus and build nothing.
use std::path::{Path, PathBuf};

/// The files a `NAME = a.c \ b.c` list of one of Opus' `.mk` files names.
fn list(root: &Path, file: &str, name: &str) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(root.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
    let mut found = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if !inside {
            let Some(rest) = line.strip_prefix(name) else { continue };
            let Some(rest) = rest.trim_start().strip_prefix('=') else { continue };
            inside = true;
            push(&mut found, rest);
        } else {
            push(&mut found, line);
        }
        if inside && !line.ends_with('\\') {
            break;
        }
    }
    assert!(!found.is_empty(), "{file}: no {name}");
    found
}

fn push(found: &mut Vec<PathBuf>, line: &str) {
    let line = line.trim_end_matches('\\').trim();
    if !line.is_empty() {
        found.push(PathBuf::from(line));
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("android") {
        return;
    }
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("opus");
    if !root.join("include/opus.h").is_file() {
        panic!("opus/ is missing: run packaging/android/fetch_opus.sh (it downloads xiph/opus v1.5.2)");
    }
    println!("cargo:rerun-if-changed=opus");
    let mut files = Vec::new();
    files.extend(list(&root, "celt_sources.mk", "CELT_SOURCES"));
    files.extend(list(&root, "silk_sources.mk", "SILK_SOURCES"));
    files.extend(list(&root, "silk_sources.mk", "SILK_SOURCES_FLOAT"));
    files.extend(list(&root, "opus_sources.mk", "OPUS_SOURCES"));
    files.extend(list(&root, "opus_sources.mk", "OPUS_SOURCES_FLOAT"));
    let mut build = cc::Build::new();
    build
        .warnings(false)
        .opt_level(3)
        .define("OPUS_BUILD", None)
        .define("VAR_ARRAYS", None)
        .define("HAVE_LRINTF", "1")
        .define("HAVE_LRINT", "1")
        .define("PACKAGE_VERSION", "\"1.5.2\"");
    for dir in ["include", "celt", "silk", "silk/float", "src"] {
        build.include(root.join(dir));
    }
    for file in files {
        build.file(root.join(file));
    }
    build.compile("opus_voicechat");
}
