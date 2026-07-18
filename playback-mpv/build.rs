use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

const WINDOWS_ARTIFACT: &str = "windows-x86_64-v3-dynamic";
const WINDOWS_TARGET: &str = "x86_64-pc-windows-msvc";
const WINDOWS_CPU_BASELINE: &str = "x86-64-v3";
const CLIENT_API_MAJOR: u32 = 2;
const CLIENT_API_MINOR: u32 = 5;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MpvLock {
    linkage: String,
    client_api: ClientApi,
    distribution: Distribution,
    licenses: BTreeMap<String, RemoteFile>,
    artifacts: BTreeMap<String, RuntimeArtifact>,
}

#[derive(Deserialize)]
struct ClientApi {
    major: u32,
    minor: u32,
}

#[derive(Deserialize)]
struct Distribution {
    asset: String,
    url: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeArtifact {
    target: String,
    architecture: String,
    cpu_baseline: String,
    link_name: String,
    import_library: PinnedFile,
    runtime_library: PinnedFile,
}

#[derive(Deserialize)]
struct PinnedFile {
    file: String,
    sha256: String,
}

#[derive(Deserialize)]
struct RemoteFile {
    url: String,
    sha256: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-env-changed=STREMIO_MPV_DIR");

    let target_os = env::var("CARGO_CFG_TARGET_OS")?;
    if target_os == "windows" {
        configure_windows_runtime()
    } else {
        configure_system_runtime()
    }
}

fn configure_windows_runtime() -> Result<(), Box<dyn std::error::Error>> {
    let target = env::var("TARGET")?;
    if target != WINDOWS_TARGET {
        return Err(io::Error::other(format!(
            "the pinned optimized MPV runtime supports {WINDOWS_TARGET}; target was {target}"
        ))
        .into());
    }

    let manifest_dir = manifest_dir()?;
    let lock_path = manifest_dir.join("../mpv.lock.json");
    println!("cargo:rerun-if-changed={}", lock_path.display());
    let lock: MpvLock =
        serde_json::from_str(&fs::read_to_string(&lock_path)?).map_err(|error| {
            io::Error::other(format!("invalid MPV lock {}: {error}", lock_path.display()))
        })?;
    validate_windows_lock(&lock)?;
    let artifact = lock.artifacts.get(WINDOWS_ARTIFACT).ok_or_else(|| {
        io::Error::other(format!(
            "MPV lock does not contain the {WINDOWS_ARTIFACT} artifact"
        ))
    })?;

    let profile_dir = cargo_profile_dir()?;
    let cache_dir = mpv_cache_dir(&lock, &profile_dir);
    let sdk_dir = match env::var_os("STREMIO_MPV_DIR") {
        Some(directory) => PathBuf::from(directory),
        None => ensure_cached_sdk(&lock, artifact, &cache_dir)?,
    };
    let import_library = resolve_pinned_file(&sdk_dir, &artifact.import_library, "import library")?;
    let runtime_library = resolve_pinned_file(&sdk_dir, &artifact.runtime_library, "runtime DLL")?;
    validate_file_hash(&import_library, &artifact.import_library.sha256)?;
    validate_file_hash(&runtime_library, &artifact.runtime_library.sha256)?;

    let library_dir = import_library.parent().ok_or_else(|| {
        io::Error::other(format!(
            "MPV import library has no parent: {}",
            import_library.display()
        ))
    })?;
    println!("cargo:rustc-link-search=native={}", library_dir.display());
    println!("cargo:rustc-link-lib=dylib={}", artifact.link_name);
    println!("cargo:rerun-if-changed={}", import_library.display());
    println!("cargo:rerun-if-changed={}", runtime_library.display());

    deploy_runtime_library(&runtime_library, &profile_dir)?;
    deploy_runtime_library(&runtime_library, &profile_dir.join("deps"))?;
    deploy_runtime_licenses(&lock, &cache_dir, &profile_dir)?;
    Ok(())
}

fn configure_system_runtime() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(directory) = env::var_os("STREMIO_MPV_DIR") {
        let directory = PathBuf::from(directory);
        let library_dir = if directory.join("lib").is_dir() {
            directory.join("lib")
        } else {
            directory
        };
        println!("cargo:rustc-link-search=native={}", library_dir.display());
        println!("cargo:rustc-link-lib=dylib=mpv");
        return Ok(());
    }

    pkg_config::Config::new()
        .cargo_metadata(true)
        .probe("mpv")
        .map_err(|error| {
            io::Error::other(format!(
                "could not locate a dynamic libmpv installation through pkg-config: {error}"
            ))
        })?;
    Ok(())
}

fn validate_windows_lock(lock: &MpvLock) -> Result<(), io::Error> {
    if lock.linkage != "dynamic" {
        return Err(io::Error::other(format!(
            "MPV lock requests {} linkage; dynamic linkage is required",
            lock.linkage
        )));
    }
    if (lock.client_api.major, lock.client_api.minor) != (CLIENT_API_MAJOR, CLIENT_API_MINOR) {
        return Err(io::Error::other(format!(
            "MPV client API pin mismatch: build expects {CLIENT_API_MAJOR}.{CLIENT_API_MINOR}, lock contains {}.{}",
            lock.client_api.major, lock.client_api.minor
        )));
    }
    validate_sha256(&lock.distribution.sha256)?;
    let url = reqwest::Url::parse(&lock.distribution.url)
        .map_err(|error| io::Error::other(format!("invalid MPV download URL: {error}")))?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return Err(io::Error::other(
            "the pinned MPV archive must use an HTTPS github.com release URL",
        ));
    }
    if Path::new(&lock.distribution.asset)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(lock.distribution.asset.as_str())
    {
        return Err(io::Error::other(
            "the pinned MPV asset must be a plain file name",
        ));
    }

    let artifact = lock
        .artifacts
        .get(WINDOWS_ARTIFACT)
        .ok_or_else(|| io::Error::other(format!("missing MPV artifact {WINDOWS_ARTIFACT}")))?;
    if artifact.target != WINDOWS_TARGET
        || artifact.architecture != "x86_64"
        || artifact.cpu_baseline != WINDOWS_CPU_BASELINE
    {
        return Err(io::Error::other(format!(
            "the MPV artifact must target {WINDOWS_TARGET} with the {WINDOWS_CPU_BASELINE} baseline"
        )));
    }
    validate_link_name(&artifact.link_name)?;
    validate_sha256(&artifact.import_library.sha256)?;
    validate_sha256(&artifact.runtime_library.sha256)?;

    for name in ["LICENSE.GPL", "LICENSE.LGPL"] {
        let license = lock
            .licenses
            .get(name)
            .ok_or_else(|| io::Error::other(format!("missing pinned MPV {name}")))?;
        validate_sha256(&license.sha256)?;
        let url = reqwest::Url::parse(&license.url)
            .map_err(|error| io::Error::other(format!("invalid MPV {name} URL: {error}")))?;
        if url.scheme() != "https" || url.host_str() != Some("raw.githubusercontent.com") {
            return Err(io::Error::other(format!(
                "the pinned MPV {name} must use an HTTPS raw.githubusercontent.com URL"
            )));
        }
    }
    Ok(())
}

fn ensure_cached_sdk(
    lock: &MpvLock,
    artifact: &RuntimeArtifact,
    cache_dir: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let cached_import = join_safe(&cache_dir, &artifact.import_library.file, "import library")?;
    let cached_runtime = join_safe(&cache_dir, &artifact.runtime_library.file, "runtime DLL")?;
    if hash_matches(&cached_import, &artifact.import_library.sha256)?
        && hash_matches(&cached_runtime, &artifact.runtime_library.sha256)?
    {
        return Ok(cache_dir.to_path_buf());
    }

    fs::create_dir_all(&cache_dir)?;
    let archive = cache_dir.join(&lock.distribution.asset);
    if !hash_matches(&archive, &lock.distribution.sha256)? {
        download_verified(&lock.distribution.url, &archive, &lock.distribution.sha256)?;
    }

    let extract_dir = cache_dir.join(format!(".extract-{}", std::process::id()));
    if extract_dir.exists() {
        fs::remove_dir_all(&extract_dir)?;
    }
    fs::create_dir_all(&extract_dir)?;
    let extraction_result = (|| -> Result<(), Box<dyn std::error::Error>> {
        sevenz_rust::decompress_file(&archive, &extract_dir).map_err(|error| {
            io::Error::other(format!(
                "failed to extract pinned MPV archive {}: {error}",
                archive.display()
            ))
        })?;

        let source_runtime = extract_dir.join("libmpv-2.dll");
        let source_import = extract_dir.join("libmpv.dll.a");
        validate_file_hash(&source_runtime, &artifact.runtime_library.sha256)?;
        validate_file_hash(&source_import, &artifact.import_library.sha256)?;
        copy_if_changed(&source_runtime, &cached_runtime)?;
        copy_if_changed(&source_import, &cached_import)?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&extract_dir);
    extraction_result?;

    validate_file_hash(&cached_import, &artifact.import_library.sha256)?;
    validate_file_hash(&cached_runtime, &artifact.runtime_library.sha256)?;
    Ok(cache_dir.to_path_buf())
}

fn deploy_runtime_licenses(
    lock: &MpvLock,
    cache_dir: &Path,
    profile_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let cached_dir = cache_dir.join("licenses");
    let destination_dir = profile_dir.join("licenses").join("mpv");
    fs::create_dir_all(&cached_dir)?;
    fs::create_dir_all(&destination_dir)?;

    for name in ["LICENSE.GPL", "LICENSE.LGPL"] {
        let license = lock
            .licenses
            .get(name)
            .ok_or_else(|| io::Error::other(format!("missing pinned MPV {name}")))?;
        let cached = join_safe(&cached_dir, name, "license")?;
        if !hash_matches(&cached, &license.sha256)? {
            download_verified(&license.url, &cached, &license.sha256)?;
        }
        let destination = destination_dir.join(name);
        copy_if_changed(&cached, &destination)?;
        println!("cargo:rerun-if-changed={}", cached.display());
        println!("cargo:rerun-if-changed={}", destination.display());
    }
    Ok(())
}

fn download_verified(url: &str, destination: &Path, expected: &str) -> Result<(), io::Error> {
    let parent = destination.parent().ok_or_else(|| {
        io::Error::other(format!(
            "download destination has no parent: {}",
            destination.display()
        ))
    })?;
    fs::create_dir_all(parent)?;
    let temporary = destination.with_extension(format!("download-{}.tmp", std::process::id()));
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }

    println!("cargo:warning=Downloading pinned optimized libmpv runtime from {url}");
    let client = reqwest::blocking::Client::builder()
        .user_agent("stremio-native-build")
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|error| io::Error::other(format!("failed to create HTTP client: {error}")))?;
    let mut response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| io::Error::other(format!("failed to download MPV runtime: {error}")))?;
    let mut file = fs::File::create(&temporary)?;
    io::copy(&mut response, &mut file)?;
    file.flush()?;
    drop(file);
    if let Err(error) = validate_file_hash(&temporary, expected) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(&temporary, destination).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        error
    })
}

fn deploy_runtime_library(source: &Path, destination_dir: &Path) -> Result<(), io::Error> {
    fs::create_dir_all(destination_dir)?;
    let file_name = source.file_name().ok_or_else(|| {
        io::Error::other(format!(
            "MPV runtime has no file name: {}",
            source.display()
        ))
    })?;
    let destination = destination_dir.join(file_name);
    copy_if_changed(source, &destination)?;
    println!("cargo:rerun-if-changed={}", destination.display());
    Ok(())
}

fn resolve_pinned_file(
    sdk_dir: &Path,
    file: &PinnedFile,
    description: &str,
) -> Result<PathBuf, io::Error> {
    let path = join_safe(sdk_dir, &file.file, description)?;
    if !path.is_file() {
        return Err(io::Error::other(format!(
            "MPV {description} is missing: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn join_safe(base: &Path, relative: &str, description: &str) -> Result<PathBuf, io::Error> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::other(format!(
            "MPV {description} path must remain inside its SDK: {relative:?}"
        )));
    }
    Ok(base.join(relative))
}

fn copy_if_changed(source: &Path, destination: &Path) -> Result<(), io::Error> {
    let source_hash = sha256_file(source)?;
    if hash_matches(destination, &source_hash)? {
        return Ok(());
    }
    let parent = destination.parent().ok_or_else(|| {
        io::Error::other(format!(
            "copy destination has no parent: {}",
            destination.display()
        ))
    })?;
    fs::create_dir_all(parent)?;
    let temporary = destination.with_extension(format!("copy-{}.tmp", std::process::id()));
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }
    fs::copy(source, &temporary)?;
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(&temporary, destination).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        error
    })
}

fn hash_matches(path: &Path, expected: &str) -> Result<bool, io::Error> {
    match sha256_file(path) {
        Ok(actual) => Ok(actual.eq_ignore_ascii_case(expected)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn validate_file_hash(path: &Path, expected: &str) -> Result<(), io::Error> {
    let actual = sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "SHA-256 mismatch for {}: expected {expected}, got {actual}",
            path.display()
        )))
    }
}

fn sha256_file(path: &Path) -> Result<String, io::Error> {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

    let mut reader = BufReader::new(fs::File::open(path)?);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest.iter().copied() {
        encoded.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    Ok(encoded)
}

fn validate_sha256(value: &str) -> Result<(), io::Error> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "invalid SHA-256 value: {value:?}"
        )))
    }
}

fn validate_link_name(name: &str) -> Result<(), io::Error> {
    if !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        Ok(())
    } else {
        Err(io::Error::other(format!("invalid MPV link name: {name:?}")))
    }
}

fn manifest_dir() -> Result<PathBuf, io::Error> {
    env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("Cargo did not provide CARGO_MANIFEST_DIR"))
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
                "could not derive Cargo's profile directory from {}",
                out_dir.display()
            ))
        })
}

fn mpv_cache_dir(lock: &MpvLock, profile_dir: &Path) -> PathBuf {
    let cache_root = profile_dir
        .parent()
        .unwrap_or(profile_dir)
        .join(".mpv-cache");
    cache_root.join(&lock.distribution.sha256[..16])
}
