//! Builds `bomb-dictate`, the Swift dictation helper (Apple's SpeechAnalyzer has no C or
//! Objective-C API), and puts it next to the bomb_app binary. Dictation is optional: without
//! Xcode's Swift compiler or a macOS 26 SDK the helper is skipped and the mic button stays hidden.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let sources = [root.join("ios/BombCode/Speech/Dictation.swift"), root.join("tools/bomb-dictate/main.swift")];
    for source in &sources {
        println!("cargo:rerun-if-changed={}", source.display());
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
    let helper = bin_dir.join("bomb-dictate");
    let built = out.join("bomb-dictate");
    let result = Command::new("xcrun")
        .args(["swiftc", "-O", "-target", &format!("{arch}-apple-macos13.0"), "-o"])
        .arg(&built)
        .args(&sources)
        .output();
    match result {
        Ok(o) if o.status.success() => {
            if let Err(e) = std::fs::copy(&built, &helper) {
                println!("cargo:warning=dictation helper built but not copied: {e}");
            }
        }
        Ok(o) => {
            let _ = std::fs::remove_file(&helper);
            let reason = String::from_utf8_lossy(&o.stderr);
            let first = reason.lines().find(|l| l.contains("error")).unwrap_or("swiftc failed");
            println!("cargo:warning=dictation helper skipped: {first}");
        }
        Err(e) => {
            let _ = std::fs::remove_file(&helper);
            println!("cargo:warning=dictation helper skipped (no Swift compiler): {e}");
        }
    }
}
