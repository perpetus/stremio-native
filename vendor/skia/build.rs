use std::{fs, io, path::Path};

#[cfg(windows)]
use std::{
    ffi::OsString,
    path::{MAIN_SEPARATOR, PathBuf},
    process::Command,
};

use build_support::{
    binaries_config,
    cargo::{self, Target},
    features, platform, skia, skia_bindgen,
};

mod build_support;

fn main() -> Result<(), io::Error> {
    if env::is_docs_rs_build() {
        println!("DETECTED DOCS_RS BUILD");
        return fake_bindings();
    }

    configure_windows_build()?;

    let skia_debug = env::is_skia_debug();
    let cargo_target = cargo::target();

    let features = {
        let mut features = features::Features::from_cargo_env();
        let missing_dependencies = features.missing_dependencies();
        if !missing_dependencies.is_empty() {
            return Err(io::Error::other(format!(
                "Missing dependent features: {missing_dependencies}"
            )));
        }

        let redundant_features = platform::redundant_features(&features, &cargo_target);
        if !redundant_features.is_empty() {
            #[cfg(feature = "binary-cache")]
            if build_support::binary_cache::should_export().is_some() {
                return Err(io::Error::other(format!(
                    "Can't produce binaries with redundant features: {redundant_features}"
                )));
            }
            cargo::warning(format!(
                "Redundant features: {redundant_features}. Disabled for the build and binary download."
            ));
            features -= redundant_features;
            cargo::warning(format!("Final features: {features}"));
        }

        features
    };

    let binaries_config =
        binaries_config::BinariesConfiguration::from_features(&features, skia_debug);

    //
    // skip attempting to download?
    //
    if let Some(source_dir) = env::source_dir() {
        if let Some(search_path) = env::skia_lib_search_path() {
            println!("STARTING BIND AGAINST SYSTEM SKIA");

            binaries_config.import(&search_path, false).unwrap();

            let definitions = skia_bindgen::definitions::from_env();
            generate_bindings(
                &features,
                definitions,
                &binaries_config,
                &source_dir,
                cargo_target,
                None,
            );
        } else {
            if cfg!(feature = "no-compile") {
                panic!("Refusing to offline-build skia with no-compile feature");
            }

            println!("STARTING OFFLINE BUILD");

            let final_build_configuration = build_from_source(
                features.clone(),
                &binaries_config,
                &source_dir,
                skia_debug,
                true,
            );
            let definitions = skia_bindgen::definitions::from_ninja_features(
                &features,
                final_build_configuration.use_system_libraries,
                &binaries_config.output_directory,
            );
            generate_bindings(
                &features,
                definitions,
                &binaries_config,
                &source_dir,
                final_build_configuration.target,
                final_build_configuration
                    .sysroot
                    .as_ref()
                    .map(AsRef::as_ref),
            );
        }
    } else {
        //
        // is the download of prebuilt binaries possible?
        //

        #[allow(unused_variables)]
        let build_skia = true;

        #[cfg(feature = "binary-cache")]
        let build_skia = build_support::binary_cache::try_prepare_download(&binaries_config);

        //
        // full build?
        //

        if build_skia {
            if cfg!(feature = "no-compile") {
                panic!("Refusing to full-build skia with no-compile feature");
            }

            println!("STARTING A FULL BUILD");
            println!("HOST: {}", cargo::host());

            let source_root = ShortSourceRoot::new(&std::env::current_dir()?)?;
            let source_dir = source_root.path().join("skia");
            let final_build_configuration = build_from_source(
                features.clone(),
                &binaries_config,
                &source_dir,
                skia_debug,
                false,
            );
            let definitions = skia_bindgen::definitions::from_ninja_features(
                &features,
                final_build_configuration.use_system_libraries,
                &binaries_config.output_directory,
            );
            generate_bindings(
                &features,
                definitions,
                &binaries_config,
                &source_dir,
                final_build_configuration.target,
                final_build_configuration
                    .sysroot
                    .as_ref()
                    .map(AsRef::as_ref),
            );
        }
    };

    binaries_config.commit_to_cargo();

    #[cfg(feature = "binary-cache")]
    if let Some(staging_directory) = build_support::binary_cache::should_export() {
        build_support::binary_cache::publish(&binaries_config, &staging_directory);
    }

    Ok(())
}

struct ShortSourceRoot {
    path: std::path::PathBuf,
    #[cfg(windows)]
    mapped_drive: Option<String>,
}

impl ShortSourceRoot {
    #[cfg(not(windows))]
    fn new(path: &Path) -> io::Result<Self> {
        Ok(Self {
            path: path.to_owned(),
        })
    }

    #[cfg(windows)]
    fn new(path: &Path) -> io::Result<Self> {
        const CANDIDATE_DRIVES: [char; 23] = [
            'S', 'R', 'Q', 'P', 'O', 'N', 'M', 'L', 'K', 'J', 'Z', 'Y', 'X', 'W', 'V', 'U', 'T',
            'I', 'H', 'G', 'F', 'E', 'D',
        ];

        for letter in CANDIDATE_DRIVES {
            let drive = format!("{letter}:");
            if Path::new(&format!("{drive}{MAIN_SEPARATOR}")).exists() {
                continue;
            }

            let status = Command::new("subst.exe").arg(&drive).arg(path).status();
            if matches!(status, Ok(status) if status.success()) {
                return Ok(Self {
                    path: PathBuf::from(format!("{drive}{MAIN_SEPARATOR}")),
                    mapped_drive: Some(drive),
                });
            }
        }

        Err(io::Error::other(
            "rust-skia needs a short source path on Windows, but no free drive letter could be mapped",
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(windows)]
impl Drop for ShortSourceRoot {
    fn drop(&mut self) {
        let Some(drive) = self.mapped_drive.take() else {
            return;
        };
        if !matches!(
            Command::new("subst.exe").args([drive.as_str(), "/d"]).status(),
            Ok(status) if status.success()
        ) {
            cargo::warning(format!(
                "could not remove temporary rust-skia drive mapping {drive}"
            ));
        }
    }
}

#[cfg(not(windows))]
fn configure_windows_build() -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn configure_windows_build() -> io::Result<()> {
    println!("cargo:rerun-if-env-changed=SKIA_GN_ARGS");
    println!("cargo:rerun-if-env-changed=BINDGEN_EXTRA_CLANG_ARGS");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return Ok(());
    }

    let sdk = complete_windows_sdk()?;
    let mut gn_args = std::env::var("SKIA_GN_ARGS").unwrap_or_default();
    if !gn_args.contains("win_sdk_version") {
        if !gn_args.is_empty() {
            gn_args.push(' ');
        }
        gn_args.push_str(&format!("win_sdk_version=\"{}\"", sdk.version));
    }

    let mut bindgen_args = std::env::var("BINDGEN_EXTRA_CLANG_ARGS").unwrap_or_default();
    for directory in ["ucrt", "shared", "um", "winrt"] {
        if !bindgen_args.is_empty() {
            bindgen_args.push(' ');
        }
        bindgen_args.push_str(&format!(
            "-isystem \"{}\"",
            sdk.include.join(directory).display()
        ));
    }

    let sdk_root = path_with_trailing_separator(&sdk.root);
    let sdk_version = format!("{}{}", sdk.version, MAIN_SEPARATOR);

    // SAFETY: This build script is single-threaded here and has not spawned any
    // worker threads. These variables configure only its later child processes.
    unsafe {
        std::env::set_var("SKIA_GN_ARGS", gn_args);
        std::env::set_var("BINDGEN_EXTRA_CLANG_ARGS", bindgen_args);
        std::env::set_var("WindowsSdkDir", sdk_root);
        std::env::set_var("WindowsSDKVersion", sdk_version);
    }

    println!("Using complete Windows SDK {} for rust-skia", sdk.version);
    Ok(())
}

#[cfg(windows)]
struct WindowsSdk {
    root: PathBuf,
    include: PathBuf,
    version: String,
}

#[cfg(windows)]
fn complete_windows_sdk() -> io::Result<WindowsSdk> {
    let program_files = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));
    let root = program_files.join(r"Windows Kits\10");
    let include_root = root.join("Include");
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").map_err(io::Error::other)?;
    let target_arch = match target_arch.as_str() {
        "x86_64" => "x64",
        "x86" => "x86",
        "aarch64" => "arm64",
        architecture => {
            return Err(io::Error::other(format!(
                "unsupported Windows SDK architecture for rust-skia: {architecture}"
            )));
        }
    };

    let mut candidates = fs::read_dir(&include_root)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let version = entry.file_name().into_string().ok()?;
            let parsed = parse_sdk_version(&version)?;
            let include = entry.path();
            let library = root.join("Lib").join(&version);
            let complete = include.join(r"ucrt\stdio.h").is_file()
                && include.join(r"shared\windef.h").is_file()
                && include.join(r"um\Windows.h").is_file()
                && include.join("winrt").is_dir()
                && library
                    .join("ucrt")
                    .join(target_arch)
                    .join("ucrt.lib")
                    .is_file()
                && library
                    .join("um")
                    .join(target_arch)
                    .join("kernel32.lib")
                    .is_file();
            complete.then_some((parsed, version, include))
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|(version, _, _)| *version);

    let Some((_, version, include)) = candidates.pop() else {
        return Err(io::Error::other(format!(
            "no complete Windows SDK for {target_arch} was found under {}",
            root.display()
        )));
    };

    Ok(WindowsSdk {
        root,
        include,
        version,
    })
}

#[cfg(windows)]
fn parse_sdk_version(version: &str) -> Option<[u32; 4]> {
    let mut parts = version.split('.').map(str::parse::<u32>);
    let parsed = [
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    ];
    parts.next().is_none().then_some(parsed)
}

#[cfg(windows)]
fn path_with_trailing_separator(path: &Path) -> OsString {
    let mut value = path.as_os_str().to_owned();
    if !path.as_os_str().to_string_lossy().ends_with(MAIN_SEPARATOR) {
        value.push(MAIN_SEPARATOR.to_string());
    }
    value
}

fn build_from_source(
    features: features::Features,
    binaries_config: &binaries_config::BinariesConfiguration,
    skia_source_dir: &std::path::Path,
    skia_debug: bool,
    offline: bool,
) -> skia::FinalBuildConfiguration {
    let build_config = skia::BuildConfiguration::from_features(features, skia_debug);
    let final_configuration = skia::FinalBuildConfiguration::from_build_configuration(
        &build_config,
        skia::env::use_system_libraries(),
        skia_source_dir,
    );

    skia::build(
        &final_configuration,
        binaries_config,
        skia::env::ninja_command(),
        skia::env::gn_command(),
        offline,
    );

    final_configuration
}

fn generate_bindings(
    features: &features::Features,
    definitions: Vec<skia_bindgen::Definition>,
    binaries_config: &binaries_config::BinariesConfiguration,
    skia_source_dir: &std::path::Path,
    target: Target,
    sysroot: Option<&str>,
) {
    // Emit the ninja definitions, to help debug build consistency.
    skia_bindgen::definitions::save_definitions(&definitions, &binaries_config.output_directory)
        .expect("failed to write Skia defines");

    let bindings_config = skia_bindgen::Configuration::new(features, definitions, skia_source_dir);
    skia_bindgen::generate_bindings(
        &bindings_config,
        &binaries_config.output_directory,
        target,
        sysroot,
    );
}

/// On docs.rs, rustdoc runs inside a container with no networking, so copy a pre-generated
/// `bindings.rs` file.
fn fake_bindings() -> Result<(), io::Error> {
    let source = std::path::Path::new("bindings_docs.rs");
    if !source.exists() {
        return Err(io::Error::other(
            "bindings_docs.rs is missing from the published skia-bindings \
             tarball. The release Makefile must run `make publish-bindings-docs` \
             (not `make publish-bindings`) so the documentation bindings end up \
             in the tarball. Without this file, docs.rs builds of skia-safe and \
             every downstream crate fail here. See \
             https://github.com/rust-skia/rust-skia/issues/720",
        ));
    }
    println!("COPYING bindings_docs.rs to OUT_DIR/skia/bindings.rs");
    let bindings_parent = cargo::output_directory().join(binaries_config::SKIA_OUTPUT_DIR);
    fs::create_dir_all(&bindings_parent)?;
    fs::copy(source, bindings_parent.join("bindings.rs")).map(|_| ())
}

/// Environment variables used by this build script.
mod env {
    use crate::build_support::cargo;
    use std::path::PathBuf;

    /// The path to the Skia source directory.
    pub fn source_dir() -> Option<PathBuf> {
        cargo::env_var("SKIA_SOURCE_DIR").map(PathBuf::from)
    }

    /// The path to where a pre-built Skia library can be found.
    pub fn skia_lib_search_path() -> Option<PathBuf> {
        cargo::env_var("SKIA_LIBRARY_SEARCH_PATH").map(PathBuf::from)
    }

    pub fn is_skia_debug() -> bool {
        matches!(cargo::env_var("SKIA_DEBUG"), Some(v) if v != "0")
    }

    pub fn is_docs_rs_build() -> bool {
        matches!(cargo::env_var("DOCS_RS"), Some(v) if v != "0")
    }
}
