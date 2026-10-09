//! Builds the Swift helpers Apple's speech APIs need and puts them next to the bomb_app binary:
//! `bomb-dictate` (SpeechAnalyzer has no C or Objective-C API) and `bomb-speak` (read-aloud,
//! sharing its engine with the iPhone app). Both are optional: without Xcode's Swift compiler or
//! a recent enough SDK a helper is skipped, and its feature stays hidden in the app.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let helpers: [(&str, Vec<PathBuf>); 2] = [
        ("bomb-dictate", vec![root.join("ios/BombCode/Speech/Dictation.swift"), root.join("tools/bomb-dictate/main.swift")]),
        ("bomb-speak", vec![root.join("ios/BombCode/Speech/SpeechEngine.swift"), root.join("tools/bomb-speak/main.swift")]),
    ];
    for (_, sources) in &helpers {
        for source in sources {
            println!("cargo:rerun-if-changed={}", source.display());
        }
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => return,
    };
    // OUT_DIR is target/<profile>/build/bomb_app-<hash>/out; the binary sits in target/<profile>.
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let Some(bin_dir) = out.ancestors().nth(3) else { return };
    for (name, sources) in &helpers {
        build(name, sources, arch, &out, bin_dir);
    }
}

fn build(name: &str, sources: &[PathBuf], arch: &str, out: &Path, bin_dir: &Path) {
    let helper = bin_dir.join(name);
    let built = out.join(name);
    let result = Command::new("xcrun")
        .args(["swiftc", "-O", "-target", &format!("{arch}-apple-macos13.0"), "-o"])
        .arg(&built)
        .args(sources)
        .output();
    match result {
        Ok(o) if o.status.success() => {
            // A new file, swapped in: copying over the old one in place keeps macOS's cached code
            // signature for it, and the new binary is killed as soon as it starts.
            let staged = helper.with_extension("new");
            if let Err(e) = std::fs::copy(&built, &staged).and_then(|_| std::fs::rename(&staged, &helper)) {
                println!("cargo:warning={name} built but not copied: {e}");
            }
        }
        Ok(o) => {
            let _ = std::fs::remove_file(&helper);
            let reason = String::from_utf8_lossy(&o.stderr);
            let first = reason.lines().find(|l| l.contains("error")).unwrap_or("swiftc failed");
            println!("cargo:warning={name} skipped: {first}");
        }
        Err(e) => {
            let _ = std::fs::remove_file(&helper);
            println!("cargo:warning={name} skipped (no Swift compiler): {e}");
        }
    }
}
