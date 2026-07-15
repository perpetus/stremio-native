use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=STREMIO_MPV_STATIC_DIR");

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo did not provide target OS");
    let target_arch =
        env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo did not provide target architecture");
    let target_env = env::var("CARGO_CFG_TARGET_ENV").expect("Cargo did not provide target ABI");
    if (
        target_os.as_str(),
        target_arch.as_str(),
        target_env.as_str(),
    ) != ("windows", "x86_64", "msvc")
    {
        panic!(
            "the bundled MPV SDK currently supports only x86_64-pc-windows-msvc; target was {target_arch}-{target_os}-{target_env}"
        );
    }

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let sdk_dir = env::var_os("STREMIO_MPV_STATIC_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../dist/mpv/windows-x86_64-static"));
    let link_manifest = sdk_dir.join("link-libraries.txt");
    println!("cargo:rerun-if-changed={}", link_manifest.display());

    let entries = fs::read_to_string(&link_manifest).unwrap_or_else(|error| {
        panic!(
            "the statically linked MPV SDK is missing at {} ({error}); run scripts/sync-mpv.ps1 first",
            sdk_dir.display()
        )
    });
    let entries = entries.strip_prefix('\u{FEFF}').unwrap_or(&entries);
    let library_dir = sdk_dir.join("lib");
    println!("cargo:rustc-link-search=native={}", library_dir.display());

    for (index, line) in entries.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (kind, value) = line.split_once('|').unwrap_or_else(|| {
            panic!(
                "invalid MPV link manifest entry on line {}: {line}",
                index + 1
            )
        });
        match kind {
            "static" => {
                let archive = library_dir.join(format!("{value}.lib"));
                assert!(
                    archive.is_file(),
                    "MPV static archive is missing: {}",
                    archive.display()
                );
                println!("cargo:rerun-if-changed={}", archive.display());
                println!("cargo:rustc-link-lib=static={value}");
            }
            "whole-static" => {
                let archive = library_dir.join(format!("{value}.lib"));
                assert!(
                    archive.is_file(),
                    "MPV whole-archive dependency is missing: {}",
                    archive.display()
                );
                println!("cargo:rerun-if-changed={}", archive.display());
                println!("cargo:rustc-link-lib=static:+whole-archive={value}");
            }
            "system" => println!("cargo:rustc-link-lib=dylib={value}"),
            "link-arg" => println!("cargo:rustc-link-arg={value}"),
            _ => panic!(
                "unknown MPV link manifest kind on line {}: {kind}",
                index + 1
            ),
        }
    }
}
