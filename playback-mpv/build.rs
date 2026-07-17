use std::{
    env, fs,
    io::{self, BufReader, Read},
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;

const SUPPORTED_TARGET: &str = "x86_64-pc-windows-msvc";
const SUPPORTED_CPU_BASELINE: &str = "x86-64-v3";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeSdk {
    schema_version: u32,
    target: String,
    cpu_baseline: String,
    link_name: String,
    import_library: SdkFile,
    runtime_library: SdkFile,
}

#[derive(Deserialize)]
struct SdkFile {
    file: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=STREMIO_MPV_DIR");

    let target = env::var("TARGET")?;
    if target != SUPPORTED_TARGET {
        return Err(io::Error::other(format!(
            "the bundled MPV runtime supports only {SUPPORTED_TARGET}; target was {target}"
        ))
        .into());
    }

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| io::Error::other("Cargo did not provide CARGO_MANIFEST_DIR"))?,
    );
    let sdk_dir = env::var_os("STREMIO_MPV_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../dist/mpv/windows-x86_64-v3-dynamic"));
    let sdk_manifest_path = sdk_dir.join("runtime-sdk.json");
    println!("cargo:rerun-if-changed={}", sdk_manifest_path.display());

    let sdk_manifest = fs::read_to_string(&sdk_manifest_path).map_err(|error| {
        io::Error::other(format!(
            "the tracked MPV runtime is missing at {} ({error}); restore dist/mpv/windows-x86_64-v3-dynamic or set STREMIO_MPV_DIR",
            sdk_dir.display()
        ))
    })?;
    let sdk_manifest = sdk_manifest
        .strip_prefix('\u{FEFF}')
        .unwrap_or(&sdk_manifest);
    let sdk: RuntimeSdk = serde_json::from_str(sdk_manifest).map_err(|error| {
        io::Error::other(format!(
            "invalid MPV runtime manifest {}: {error}",
            sdk_manifest_path.display()
        ))
    })?;

    if sdk.schema_version != 1 {
        return Err(io::Error::other(format!(
            "unsupported MPV runtime schema {} in {}",
            sdk.schema_version,
            sdk_manifest_path.display()
        ))
        .into());
    }
    if sdk.target != target {
        return Err(io::Error::other(format!(
            "MPV runtime target mismatch: manifest contains {}, Cargo requested {target}",
            sdk.target
        ))
        .into());
    }
    if sdk.cpu_baseline != SUPPORTED_CPU_BASELINE {
        return Err(io::Error::other(format!(
            "MPV runtime CPU baseline mismatch: expected {SUPPORTED_CPU_BASELINE}, manifest contains {}",
            sdk.cpu_baseline
        ))
        .into());
    }
    validate_link_name(&sdk.link_name)?;

    let import_library = resolve_sdk_file(&sdk_dir, &sdk.import_library.file, "import library")?;
    let runtime_library =
        resolve_sdk_file(&sdk_dir, &sdk.runtime_library.file, "runtime library")?;
    println!("cargo:rerun-if-changed={}", import_library.display());
    println!("cargo:rerun-if-changed={}", runtime_library.display());

    let library_dir = import_library.parent().ok_or_else(|| {
        io::Error::other(format!(
            "MPV import library has no parent directory: {}",
            import_library.display()
        ))
    })?;
    println!("cargo:rustc-link-search=native={}", library_dir.display());
    println!("cargo:rustc-link-lib=dylib={}", sdk.link_name);

    let profile_dir = cargo_profile_dir()?;
    deploy_runtime_library(&runtime_library, &profile_dir)?;
    deploy_runtime_library(&runtime_library, &profile_dir.join("deps"))?;

    Ok(())
}

fn resolve_sdk_file(
    sdk_dir: &Path,
    relative_path: &str,
    description: &str,
) -> Result<PathBuf, io::Error> {
    let relative_path = Path::new(relative_path);
    if relative_path.as_os_str().is_empty()
        || !relative_path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::other(format!(
            "MPV {description} path must stay inside the SDK: {relative_path:?}"
        )));
    }

    let path = sdk_dir.join(relative_path);
    if !path.is_file() {
        return Err(io::Error::other(format!(
            "MPV {description} is missing: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn cargo_profile_dir() -> Result<PathBuf, io::Error> {
    let out_dir = PathBuf::from(
        env::var_os("OUT_DIR").ok_or_else(|| io::Error::other("Cargo did not provide OUT_DIR"))?,
    );
    out_dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            io::Error::other(format!(
                "could not derive Cargo's profile directory from OUT_DIR {}",
                out_dir.display()
            ))
        })
}

fn deploy_runtime_library(source: &Path, destination_dir: &Path) -> Result<(), io::Error> {
    fs::create_dir_all(destination_dir)?;
    let file_name = source.file_name().ok_or_else(|| {
        io::Error::other(format!(
            "MPV runtime library has no file name: {}",
            source.display()
        ))
    })?;
    let destination = destination_dir.join(file_name);
    copy_if_changed(source, &destination).map_err(|error| {
        io::Error::other(format!(
            "failed to deploy MPV runtime from {} to {}: {error}",
            source.display(),
            destination.display()
        ))
    })
}

fn copy_if_changed(source: &Path, destination: &Path) -> Result<(), io::Error> {
    if files_equal(source, destination)? {
        return Ok(());
    }

    let destination_name = destination
        .file_name()
        .ok_or_else(|| io::Error::other("runtime destination has no file name"))?
        .to_string_lossy();
    let temporary = destination.with_file_name(format!(
        ".{destination_name}.{}.tmp",
        std::process::id()
    ));
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }

    fs::copy(source, &temporary)?;
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    if let Err(error) = fs::rename(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn files_equal(left: &Path, right: &Path) -> Result<bool, io::Error> {
    let left_metadata = fs::metadata(left)?;
    let right_metadata = match fs::metadata(right) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if left_metadata.len() != right_metadata.len() {
        return Ok(false);
    }

    let mut left = BufReader::new(fs::File::open(left)?);
    let mut right = BufReader::new(fs::File::open(right)?);
    let mut left_buffer = [0_u8; 64 * 1024];
    let mut right_buffer = [0_u8; 64 * 1024];
    loop {
        let left_read = left.read(&mut left_buffer)?;
        let right_read = right.read(&mut right_buffer)?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

fn validate_link_name(name: &str) -> Result<(), io::Error> {
    if !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Ok(());
    }

    Err(io::Error::other(format!(
        "invalid library name in MPV runtime manifest: {name:?}"
    )))
}
