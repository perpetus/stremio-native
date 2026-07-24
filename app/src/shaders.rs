use std::path::Path;

use anyhow::{Context, Result};
use playback_mpv::VideoShaderUnsupportedReason;
use tracing::{info, warn};

pub const SHADER_PRESET_COUNT: usize = 8;

const MODE_A_PATHS: &[&str] = &[
    "~~/shaders/Anime4K_Clamp_Highlights.glsl",
    "~~/shaders/Anime4K_Restore_CNN_VL.glsl",
    "~~/shaders/Anime4K_Upscale_CNN_x2_VL.glsl",
    "~~/shaders/Anime4K_AutoDownscalePre_x2.glsl",
    "~~/shaders/Anime4K_AutoDownscalePre_x4.glsl",
    "~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl",
];
const MODE_B_PATHS: &[&str] = &[
    "~~/shaders/Anime4K_Clamp_Highlights.glsl",
    "~~/shaders/Anime4K_Restore_CNN_Soft_VL.glsl",
    "~~/shaders/Anime4K_Upscale_CNN_x2_VL.glsl",
    "~~/shaders/Anime4K_AutoDownscalePre_x2.glsl",
    "~~/shaders/Anime4K_AutoDownscalePre_x4.glsl",
    "~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl",
];
const MODE_C_PATHS: &[&str] = &[
    "~~/shaders/Anime4K_Clamp_Highlights.glsl",
    "~~/shaders/Anime4K_Upscale_Denoise_CNN_x2_VL.glsl",
    "~~/shaders/Anime4K_AutoDownscalePre_x2.glsl",
    "~~/shaders/Anime4K_AutoDownscalePre_x4.glsl",
    "~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl",
];
const MODE_AA_PATHS: &[&str] = &[
    "~~/shaders/Anime4K_Clamp_Highlights.glsl",
    "~~/shaders/Anime4K_Restore_CNN_VL.glsl",
    "~~/shaders/Anime4K_Upscale_CNN_x2_VL.glsl",
    "~~/shaders/Anime4K_Restore_CNN_M.glsl",
    "~~/shaders/Anime4K_AutoDownscalePre_x2.glsl",
    "~~/shaders/Anime4K_AutoDownscalePre_x4.glsl",
    "~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl",
];
const MODE_BB_PATHS: &[&str] = &[
    "~~/shaders/Anime4K_Clamp_Highlights.glsl",
    "~~/shaders/Anime4K_Restore_CNN_Soft_VL.glsl",
    "~~/shaders/Anime4K_Upscale_CNN_x2_VL.glsl",
    "~~/shaders/Anime4K_AutoDownscalePre_x2.glsl",
    "~~/shaders/Anime4K_AutoDownscalePre_x4.glsl",
    "~~/shaders/Anime4K_Restore_CNN_Soft_M.glsl",
    "~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl",
];
const MODE_CA_PATHS: &[&str] = &[
    "~~/shaders/Anime4K_Clamp_Highlights.glsl",
    "~~/shaders/Anime4K_Upscale_Denoise_CNN_x2_VL.glsl",
    "~~/shaders/Anime4K_AutoDownscalePre_x2.glsl",
    "~~/shaders/Anime4K_AutoDownscalePre_x4.glsl",
    "~~/shaders/Anime4K_Restore_CNN_M.glsl",
    "~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl",
];
const FSR_PATHS: &[&str] = &["~~/shaders/FSR.glsl"];

const MODE_A_FILES: &[&str] = &[
    "Anime4K_Clamp_Highlights.glsl",
    "Anime4K_Restore_CNN_VL.glsl",
    "Anime4K_Upscale_CNN_x2_VL.glsl",
    "Anime4K_AutoDownscalePre_x2.glsl",
    "Anime4K_AutoDownscalePre_x4.glsl",
    "Anime4K_Upscale_CNN_x2_M.glsl",
];
const MODE_B_FILES: &[&str] = &[
    "Anime4K_Clamp_Highlights.glsl",
    "Anime4K_Restore_CNN_Soft_VL.glsl",
    "Anime4K_Upscale_CNN_x2_VL.glsl",
    "Anime4K_AutoDownscalePre_x2.glsl",
    "Anime4K_AutoDownscalePre_x4.glsl",
    "Anime4K_Upscale_CNN_x2_M.glsl",
];
const MODE_C_FILES: &[&str] = &[
    "Anime4K_Clamp_Highlights.glsl",
    "Anime4K_Upscale_Denoise_CNN_x2_VL.glsl",
    "Anime4K_AutoDownscalePre_x2.glsl",
    "Anime4K_AutoDownscalePre_x4.glsl",
    "Anime4K_Upscale_CNN_x2_M.glsl",
];
const MODE_AA_FILES: &[&str] = &[
    "Anime4K_Clamp_Highlights.glsl",
    "Anime4K_Restore_CNN_VL.glsl",
    "Anime4K_Upscale_CNN_x2_VL.glsl",
    "Anime4K_Restore_CNN_M.glsl",
    "Anime4K_AutoDownscalePre_x2.glsl",
    "Anime4K_AutoDownscalePre_x4.glsl",
    "Anime4K_Upscale_CNN_x2_M.glsl",
];
const MODE_BB_FILES: &[&str] = &[
    "Anime4K_Clamp_Highlights.glsl",
    "Anime4K_Restore_CNN_Soft_VL.glsl",
    "Anime4K_Upscale_CNN_x2_VL.glsl",
    "Anime4K_AutoDownscalePre_x2.glsl",
    "Anime4K_AutoDownscalePre_x4.glsl",
    "Anime4K_Restore_CNN_Soft_M.glsl",
    "Anime4K_Upscale_CNN_x2_M.glsl",
];
const MODE_CA_FILES: &[&str] = &[
    "Anime4K_Clamp_Highlights.glsl",
    "Anime4K_Upscale_Denoise_CNN_x2_VL.glsl",
    "Anime4K_AutoDownscalePre_x2.glsl",
    "Anime4K_AutoDownscalePre_x4.glsl",
    "Anime4K_Restore_CNN_M.glsl",
    "Anime4K_Upscale_CNN_x2_M.glsl",
];
const FSR_FILES: &[&str] = &["FSR.glsl"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ShaderPreset {
    #[default]
    Off = 0,
    ModeA = 1,
    ModeB = 2,
    ModeC = 3,
    ModeAA = 4,
    ModeBB = 5,
    ModeCA = 6,
    Fsr = 7,
}

impl ShaderPreset {
    pub const ALL: [Self; SHADER_PRESET_COUNT] = [
        Self::Off,
        Self::ModeA,
        Self::ModeB,
        Self::ModeC,
        Self::ModeAA,
        Self::ModeBB,
        Self::ModeCA,
        Self::Fsr,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::ModeA => "Anime4K Mode A",
            Self::ModeB => "Anime4K Mode B",
            Self::ModeC => "Anime4K Mode C",
            Self::ModeAA => "Anime4K Mode A+A",
            Self::ModeBB => "Anime4K Mode B+B",
            Self::ModeCA => "Anime4K Mode C+A",
            Self::Fsr => "FSR",
        }
    }

    /// MPV shader paths using `~~/`, which resolves relative to `config-dir`.
    pub const fn paths(self) -> &'static [&'static str] {
        match self {
            Self::Off => &[],
            Self::ModeA => MODE_A_PATHS,
            Self::ModeB => MODE_B_PATHS,
            Self::ModeC => MODE_C_PATHS,
            Self::ModeAA => MODE_AA_PATHS,
            Self::ModeBB => MODE_BB_PATHS,
            Self::ModeCA => MODE_CA_PATHS,
            Self::Fsr => FSR_PATHS,
        }
    }

    /// Files that must all exist before this preset can be selected.
    pub const fn required_files(self) -> &'static [&'static str] {
        match self {
            Self::Off => &[],
            Self::ModeA => MODE_A_FILES,
            Self::ModeB => MODE_B_FILES,
            Self::ModeC => MODE_C_FILES,
            Self::ModeAA => MODE_AA_FILES,
            Self::ModeBB => MODE_BB_FILES,
            Self::ModeCA => MODE_CA_FILES,
            Self::Fsr => FSR_FILES,
        }
    }

    pub fn is_ready(self, shaders_dir: &Path) -> bool {
        self.required_files()
            .iter()
            .all(|file| shaders_dir.join(file).is_file())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidShaderPreset(pub i32);

impl TryFrom<u8> for ShaderPreset {
    type Error = InvalidShaderPreset;

    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        Self::try_from(i32::from(value))
    }
}

impl TryFrom<i32> for ShaderPreset {
    type Error = InvalidShaderPreset;

    fn try_from(value: i32) -> std::result::Result<Self, Self::Error> {
        Self::ALL
            .get(value as usize)
            .copied()
            .ok_or(InvalidShaderPreset(value))
    }
}

pub fn preset_from_config(value: u8) -> ShaderPreset {
    ShaderPreset::try_from(value).unwrap_or_else(|InvalidShaderPreset(value)| {
        warn!(value, "invalid stored shader preset; falling back to Off");
        ShaderPreset::Off
    })
}

pub fn preset_from_ui(value: i32) -> ShaderPreset {
    ShaderPreset::try_from(value).unwrap_or_else(|InvalidShaderPreset(value)| {
        warn!(
            value,
            "invalid shader preset selected by UI; falling back to Off"
        );
        ShaderPreset::Off
    })
}

pub fn preset_readiness(shaders_dir: &Path) -> [bool; SHADER_PRESET_COUNT] {
    std::array::from_fn(|index| ShaderPreset::ALL[index].is_ready(shaders_dir))
}

pub fn all_anime4k_presets_ready(shaders_dir: &Path) -> bool {
    anime4k_presets_ready(&preset_readiness(shaders_dir))
}

pub fn anime4k_presets_ready(readiness: &[bool; SHADER_PRESET_COUNT]) -> bool {
    readiness[ShaderPreset::ModeA.index()..ShaderPreset::Fsr.index()]
        .iter()
        .all(|ready| *ready)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShaderContextCapability {
    Pending,
    Supported,
    Unsupported(VideoShaderUnsupportedReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderConfigurationCommand {
    pub request_id: u64,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderUiProjection {
    pub active_preset: ShaderPreset,
    pub availability: [bool; SHADER_PRESET_COUNT],
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderUpdate {
    pub command: Option<ShaderConfigurationCommand>,
    pub projection: ShaderUiProjection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InFlightRequest {
    id: u64,
    preset: ShaderPreset,
}

#[derive(Debug)]
pub struct ShaderCoordinator {
    desired: ShaderPreset,
    actual: Option<ShaderPreset>,
    capability: ShaderContextCapability,
    readiness: [bool; SHADER_PRESET_COUNT],
    rejected: [bool; SHADER_PRESET_COUNT],
    in_flight: Option<InFlightRequest>,
    next_request_id: u64,
    downloading: bool,
    download_error: Option<String>,
    rejection_message: Option<String>,
}

impl ShaderCoordinator {
    pub(crate) fn with_readiness(
        desired: ShaderPreset,
        readiness: [bool; SHADER_PRESET_COUNT],
    ) -> Self {
        Self {
            desired,
            actual: None,
            capability: ShaderContextCapability::Pending,
            readiness,
            rejected: [false; SHADER_PRESET_COUNT],
            in_flight: None,
            next_request_id: 1,
            downloading: false,
            download_error: None,
            rejection_message: None,
        }
    }

    pub fn desired_preset(&self) -> ShaderPreset {
        self.desired
    }

    pub fn initial_update(&mut self) -> ShaderUpdate {
        self.reconcile()
    }

    pub fn set_context_capability(&mut self, capability: ShaderContextCapability) -> ShaderUpdate {
        self.capability = capability;
        self.actual = None;
        self.in_flight = None;
        self.rejected.fill(false);
        self.rejection_message = None;
        self.reconcile()
    }

    pub fn context_torn_down(&mut self) -> ShaderUpdate {
        self.set_context_capability(ShaderContextCapability::Pending)
    }

    pub fn set_download_state(&mut self, downloading: bool, error: Option<String>) -> ShaderUpdate {
        self.downloading = downloading;
        self.download_error = error;
        self.reconcile()
    }

    pub fn complete_download(&mut self, shaders_dir: &Path, error: Option<String>) -> ShaderUpdate {
        let readiness = preset_readiness(shaders_dir);
        if self.readiness != readiness {
            self.readiness = readiness;
            self.rejected.fill(false);
            self.rejection_message = None;
        }
        self.downloading = false;
        self.download_error = error;
        self.reconcile()
    }

    pub fn select(&mut self, preset: ShaderPreset) -> Option<ShaderUpdate> {
        if preset != ShaderPreset::Off && !self.projection().availability[preset.index()] {
            warn!(preset = ?preset, "ignored unavailable shader preset selection");
            return None;
        }
        self.desired = preset;
        self.rejection_message = None;
        Some(self.reconcile())
    }

    pub fn configured(&mut self, request_id: u64) -> Option<ShaderUpdate> {
        let request = self.in_flight.filter(|request| request.id == request_id)?;
        self.in_flight = None;
        self.actual = Some(request.preset);
        Some(self.reconcile())
    }

    pub fn rejected(&mut self, request_id: u64, message: String) -> Option<ShaderUpdate> {
        let request = self.in_flight.filter(|request| request.id == request_id)?;
        self.in_flight = None;
        self.actual = Some(ShaderPreset::Off);
        if request.preset != ShaderPreset::Off {
            self.rejected[request.preset.index()] = true;
        }
        self.rejection_message = Some(format!(
            "Could not enable {}: {message}. Playing without upscaling.",
            request.preset.display_name()
        ));
        Some(self.reconcile())
    }

    fn target_preset(&self) -> ShaderPreset {
        match self.capability {
            ShaderContextCapability::Supported
                if self.readiness[self.desired.index()] && !self.rejected[self.desired.index()] =>
            {
                self.desired
            }
            ShaderContextCapability::Pending
            | ShaderContextCapability::Supported
            | ShaderContextCapability::Unsupported(_) => ShaderPreset::Off,
        }
    }

    fn reconcile(&mut self) -> ShaderUpdate {
        let target = self.target_preset();
        let already_pending = self
            .in_flight
            .is_some_and(|request| request.preset == target);
        let command =
            if already_pending || (self.in_flight.is_none() && self.actual == Some(target)) {
                None
            } else {
                let request_id = self.next_request_id;
                self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
                self.in_flight = Some(InFlightRequest {
                    id: request_id,
                    preset: target,
                });
                Some(ShaderConfigurationCommand {
                    request_id,
                    paths: target
                        .paths()
                        .iter()
                        .map(|path| (*path).to_owned())
                        .collect(),
                })
            };
        ShaderUpdate {
            command,
            projection: self.projection(),
        }
    }

    fn projection(&self) -> ShaderUiProjection {
        let mut availability = [false; SHADER_PRESET_COUNT];
        availability[ShaderPreset::Off.index()] = true;
        if self.capability == ShaderContextCapability::Supported {
            for preset in ShaderPreset::ALL.into_iter().skip(1) {
                let download_allows_preset = !self.downloading || preset == ShaderPreset::Fsr;
                availability[preset.index()] = download_allows_preset
                    && self.readiness[preset.index()]
                    && !self.rejected[preset.index()];
            }
        }

        let status = match self.capability {
            ShaderContextCapability::Pending => "Checking GPU support…".to_owned(),
            ShaderContextCapability::Unsupported(reason) => reason.to_string(),
            ShaderContextCapability::Supported => self.supported_status(),
        };
        ShaderUiProjection {
            active_preset: self.actual.unwrap_or(ShaderPreset::Off),
            availability,
            status,
        }
    }

    fn supported_status(&self) -> String {
        if let Some(message) = &self.rejection_message {
            return message.clone();
        }
        if self.downloading {
            return "Downloading Anime4K shaders… FSR remains available.".to_owned();
        }
        if let Some(error) = &self.download_error {
            return format!("Anime4K download failed: {error}. FSR remains available.");
        }
        if self.desired != ShaderPreset::Off && !self.readiness[self.desired.index()] {
            return format!(
                "{} files are not ready. Playing without upscaling.",
                self.desired.display_name()
            );
        }
        if self.in_flight.is_some() {
            return format!("Applying {}…", self.target_preset().display_name());
        }
        match self.actual.unwrap_or(ShaderPreset::Off) {
            ShaderPreset::Off => "Upscaling is off.".to_owned(),
            preset => format!("Active: {}", preset.display_name()),
        }
    }
}

pub const DEFAULT_MPV_CONF: &str = r#"# Offsets Text from the top
osd-margin-y=50
"#;

pub const DEFAULT_INPUT_CONF: &str = r#"# Optimized shaders for higher-end GPU
CTRL+1 no-osd change-list glsl-shaders clr ""; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Clamp_Highlights.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Restore_CNN_VL.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Upscale_CNN_x2_VL.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_AutoDownscalePre_x2.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_AutoDownscalePre_x4.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl"; show-text "Anime4K: Mode A (HQ)"
CTRL+2 no-osd change-list glsl-shaders clr ""; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Clamp_Highlights.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Restore_CNN_Soft_VL.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Upscale_CNN_x2_VL.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_AutoDownscalePre_x2.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_AutoDownscalePre_x4.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl"; show-text "Anime4K: Mode B (HQ)"
CTRL+3 no-osd change-list glsl-shaders clr ""; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Clamp_Highlights.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Upscale_Denoise_CNN_x2_VL.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_AutoDownscalePre_x2.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_AutoDownscalePre_x4.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl"; show-text "Anime4K: Mode C (HQ)"
CTRL+4 no-osd change-list glsl-shaders clr ""; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Clamp_Highlights.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Restore_CNN_VL.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Upscale_CNN_x2_VL.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Restore_CNN_M.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_AutoDownscalePre_x2.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_AutoDownscalePre_x4.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl"; show-text "Anime4K: Mode A+A (HQ)"
CTRL+5 no-osd change-list glsl-shaders clr ""; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Clamp_Highlights.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Restore_CNN_Soft_VL.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Upscale_CNN_x2_VL.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_AutoDownscalePre_x2.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_AutoDownscalePre_x4.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Restore_CNN_Soft_M.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl"; show-text "Anime4K: Mode B+B (HQ)"
CTRL+6 no-osd change-list glsl-shaders clr ""; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Clamp_Highlights.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Upscale_Denoise_CNN_x2_VL.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_AutoDownscalePre_x2.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_AutoDownscalePre_x4.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Restore_CNN_M.glsl"; no-osd change-list glsl-shaders append "~~/shaders/Anime4K_Upscale_CNN_x2_M.glsl"; show-text "Anime4K: Mode C+A (HQ)"
CTRL+7 no-osd change-list glsl-shaders clr ""; no-osd change-list glsl-shaders append "~~/shaders/FSR.glsl"; show-text "FSR"

CTRL+0 no-osd change-list glsl-shaders clr ""; show-text "GLSL shaders cleared"
"#;

/// Returns `true` if the directory contains at least one `.glsl` shader file.
pub fn has_glsl_files(shaders_dir: &Path) -> bool {
    std::fs::read_dir(shaders_dir).is_ok_and(|entries| {
        entries
            .filter_map(|e| e.ok())
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "glsl"))
    })
}

/// Returns `true` if the directory contains Anime4K `.glsl` shader files
/// (as opposed to only containing `FSR.glsl` or other non-Anime4K shaders).
pub fn has_anime4k_shaders(shaders_dir: &Path) -> bool {
    std::fs::read_dir(shaders_dir).is_ok_and(|entries| {
        entries.filter_map(|e| e.ok()).any(|entry| {
            entry
                .path()
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.starts_with("Anime4K_") && name.ends_with(".glsl"))
        })
    })
}

/// Ensures Anime4K GLSL shaders and default mpv configuration exist in the config directory.
pub fn ensure_anime4k_shaders(config_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(config_dir)?;

    let mpv_conf = config_dir.join("mpv.conf");
    if !mpv_conf.exists()
        && let Err(err) = std::fs::write(&mpv_conf, DEFAULT_MPV_CONF)
    {
        warn!(error = %err, "Failed to write default mpv.conf");
    }

    let input_conf = config_dir.join("input.conf");
    let needs_input_conf_update = match std::fs::read_to_string(&input_conf) {
        Ok(content) => content.contains("change-list glsl-shaders set"),
        Err(_) => true,
    };
    if needs_input_conf_update && let Err(err) = std::fs::write(&input_conf, DEFAULT_INPUT_CONF) {
        warn!(error = %err, "Failed to write default input.conf");
    }

    let shaders_dir = config_dir.join("shaders");
    if !shaders_dir.exists() {
        std::fs::create_dir_all(&shaders_dir)?;
    }

    // Fix previously broken extraction: if .glsl files are nested inside
    // shaders/shaders/ (from the zip archive structure), flatten them up.
    flatten_nested_shaders(&shaders_dir);

    // Ensure a minimal FSR.glsl exists for the FSR preset
    ensure_fsr_shader(&shaders_dir);

    Ok(())
}

/// If the zip archive extracted a nested `shaders/` subdirectory (containing the
/// actual `.glsl` files), move them up one level and clean up junk entries.
fn flatten_nested_shaders(shaders_dir: &Path) {
    let nested = shaders_dir.join("shaders");
    if nested.is_dir() && has_glsl_files(&nested) && !has_anime4k_shaders(shaders_dir) {
        info!("Flattening nested shaders directory");
        if let Ok(entries) = std::fs::read_dir(&nested) {
            for entry in entries.filter_map(|e| e.ok()) {
                let src = entry.path();
                if src.is_file()
                    && let Some(name) = src.file_name()
                {
                    let dest = shaders_dir.join(name);
                    if let Err(err) = std::fs::rename(&src, &dest) {
                        warn!(error = %err, src = %src.display(), "Failed to move shader file");
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&nested);
    }

    // Clean up junk files left by the zip archive inside the shaders directory
    for junk in &["input.conf", "mpv.conf", "__MACOSX"] {
        let path = shaders_dir.join(junk);
        if path.is_file() {
            let _ = std::fs::remove_file(&path);
        } else if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Writes a minimal AMD FidelityFX Super Resolution (FSR) GLSL shader if missing.
fn ensure_fsr_shader(shaders_dir: &Path) {
    let fsr_path = shaders_dir.join("FSR.glsl");
    if fsr_path.exists() {
        return;
    }
    // Minimal EASU (Edge-Adaptive Spatial Upsampling) pass from AMD FSR 1.0
    let fsr_source = r#"//!HOOK MAINPRESIZED
//!BIND HOOKED
//!DESC AMD FidelityFX Super Resolution (FSR) - EASU
//!WHEN OUTPUT.w OUTPUT.h * MAINPRESIZED.w MAINPRESIZED.h * >

// Attempt to use the GPU's native min3/max3 if available, otherwise define them.
#ifndef FSR_RCAS_PASSTHROUGH_ALPHA
#define FSR_RCAS_PASSTHROUGH_ALPHA 0
#endif

vec4 hook() {
    vec2 pp = HOOKED_pos * HOOKED_size - 0.5;
    vec2 fp = floor(pp);
    pp -= fp;

    vec2 t = fp * HOOKED_pt + HOOKED_pt * 0.5;

    vec4 a = HOOKED_tex(t + HOOKED_pt * vec2(-1, -1));
    vec4 b = HOOKED_tex(t + HOOKED_pt * vec2( 0, -1));
    vec4 c = HOOKED_tex(t + HOOKED_pt * vec2( 1, -1));
    vec4 d = HOOKED_tex(t + HOOKED_pt * vec2(-1,  0));
    vec4 e = HOOKED_tex(t + HOOKED_pt * vec2( 0,  0));
    vec4 f = HOOKED_tex(t + HOOKED_pt * vec2( 1,  0));
    vec4 g = HOOKED_tex(t + HOOKED_pt * vec2(-1,  1));
    vec4 h = HOOKED_tex(t + HOOKED_pt * vec2( 0,  1));
    vec4 i = HOOKED_tex(t + HOOKED_pt * vec2( 1,  1));

    vec4 mn = min(min(min(d, e), min(f, b)), h);
    vec4 mx = max(max(max(d, e), max(f, b)), h);

    vec2 w = 1.0 - pp;
    vec4 r = mix(mix(e, f, pp.x), mix(h, i, pp.x), pp.y);
    r = clamp(r, mn, mx);
    return r;
}
"#;
    if let Err(err) = std::fs::write(&fsr_path, fsr_source) {
        warn!(error = %err, "Failed to write FSR.glsl");
    } else {
        info!("FSR.glsl shader written to {}", fsr_path.display());
    }
}

/// Asynchronously downloads Anime4K GLSL release zip if missing shaders.
pub async fn download_shaders_if_needed(config_dir: &Path) -> Result<()> {
    let shaders_dir = config_dir.join("shaders");
    // Every preset has a distinct multipass file set. A partial Anime4K
    // installation must not make unavailable presets look ready.
    if all_anime4k_presets_ready(&shaders_dir) {
        return Ok(());
    }

    info!("Anime4K shaders missing. Downloading GLSL High-end package...");
    let url =
        "https://github.com/Tama47/Anime4K/releases/download/v4.0.1/GLSL_Windows_High-end.zip";
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
        )
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .context("Failed to send request for Anime4K shaders")?;

    if !response.status().is_success() {
        anyhow::bail!(
            "Anime4K shader download returned HTTP {}",
            response.status()
        );
    }

    let bytes = response
        .bytes()
        .await
        .context("Failed to read shader download body")?;

    tokio::task::spawn_blocking(move || -> Result<()> {
        std::fs::create_dir_all(&shaders_dir)
            .with_context(|| format!("Failed to create directory: {}", shaders_dir.display()))?;

        let cursor = std::io::Cursor::new(&bytes[..]);
        let mut archive = zip::ZipArchive::new(cursor)
            .context("Failed to parse downloaded shader zip archive")?;

        let mut extracted_count = 0;
        for i in 0..archive.len() {
            let mut file = match archive.by_index(i) {
                Ok(f) => f,
                Err(_) => continue,
            };

            let name = file.name().to_string();
            if name.contains("__MACOSX") {
                continue;
            }

            let path = std::path::Path::new(&name);
            if let Some(ext) = path.extension()
                && ext == "glsl"
                    && let Some(file_name) = path.file_name() {
                        let dest_path = shaders_dir.join(file_name);
                        let mut out_file = match std::fs::File::create(&dest_path) {
                            Ok(f) => f,
                            Err(err) => {
                                warn!(error = %err, path = %dest_path.display(), "Failed to create shader file");
                                continue;
                            }
                        };
                        if let Err(err) = std::io::copy(&mut file, &mut out_file) {
                            warn!(error = %err, path = %dest_path.display(), "Failed to write shader file");
                        } else {
                            extracted_count += 1;
                        }
                    }
        }

        ensure_fsr_shader(&shaders_dir);

        if extracted_count == 0 {
            anyhow::bail!("Shader extraction completed but 0 Anime4K shader files were written");
        }

        info!(count = extracted_count, "Anime4K shaders successfully extracted to {}", shaders_dir.display());
        Ok(())
    })
    .await
    .context("Shader extraction task panicked")?
}

#[cfg(test)]
mod tests {
    use super::{
        SHADER_PRESET_COUNT, ShaderContextCapability, ShaderCoordinator, ShaderPreset,
        VideoShaderUnsupportedReason,
    };

    fn readiness(presets: &[ShaderPreset]) -> [bool; SHADER_PRESET_COUNT] {
        let mut readiness = [false; SHADER_PRESET_COUNT];
        readiness[ShaderPreset::Off.index()] = true;
        for preset in presets {
            readiness[preset.index()] = true;
        }
        readiness
    }

    #[test]
    fn files_before_context_apply_the_persisted_preset_after_validation() {
        let mut coordinator = ShaderCoordinator::with_readiness(
            ShaderPreset::ModeA,
            readiness(&[ShaderPreset::ModeA]),
        );
        let clear = coordinator.initial_update().command.expect("initial clear");
        assert!(clear.paths.is_empty());

        let configure = coordinator
            .set_context_capability(ShaderContextCapability::Supported)
            .command
            .expect("persisted preset configuration");
        assert_eq!(configure.paths.len(), ShaderPreset::ModeA.paths().len());
    }

    #[test]
    fn context_before_files_keeps_video_plain_until_files_arrive() {
        let mut coordinator =
            ShaderCoordinator::with_readiness(ShaderPreset::ModeA, readiness(&[]));
        let clear = coordinator
            .set_context_capability(ShaderContextCapability::Supported)
            .command
            .expect("clear while files are absent");
        assert!(clear.paths.is_empty());
        coordinator.configured(clear.request_id);

        coordinator.readiness = readiness(&[ShaderPreset::ModeA]);
        let configure = coordinator
            .reconcile()
            .command
            .expect("configure after files arrive");
        assert_eq!(configure.paths, ShaderPreset::ModeA.paths());
    }

    #[test]
    fn unsupported_context_clears_without_changing_the_preference() {
        let mut coordinator =
            ShaderCoordinator::with_readiness(ShaderPreset::Fsr, readiness(&[ShaderPreset::Fsr]));
        let configure = coordinator
            .set_context_capability(ShaderContextCapability::Supported)
            .command
            .expect("configure FSR");
        coordinator.configured(configure.request_id);

        let update = coordinator.set_context_capability(ShaderContextCapability::Unsupported(
            VideoShaderUnsupportedReason::EmbeddedProfile,
        ));
        assert!(update.command.expect("fallback clear").paths.is_empty());
        assert_eq!(coordinator.desired_preset(), ShaderPreset::Fsr);
        assert_eq!(update.projection.active_preset, ShaderPreset::Off);
    }

    #[test]
    fn context_recreation_restores_a_valid_preference() {
        let mut coordinator =
            ShaderCoordinator::with_readiness(ShaderPreset::Fsr, readiness(&[ShaderPreset::Fsr]));
        let configure = coordinator
            .set_context_capability(ShaderContextCapability::Supported)
            .command
            .expect("first configuration");
        coordinator.configured(configure.request_id);

        let teardown = coordinator.context_torn_down();
        assert!(teardown.command.expect("teardown clear").paths.is_empty());
        let recreated = coordinator
            .set_context_capability(ShaderContextCapability::Supported)
            .command
            .expect("restored configuration");
        assert_eq!(recreated.paths, ShaderPreset::Fsr.paths());
    }

    #[test]
    fn download_failure_leaves_off_and_ready_fsr_available() {
        let mut coordinator =
            ShaderCoordinator::with_readiness(ShaderPreset::ModeA, readiness(&[ShaderPreset::Fsr]));
        coordinator.set_context_capability(ShaderContextCapability::Supported);
        let update = coordinator.set_download_state(false, Some("network unavailable".to_owned()));

        assert!(update.projection.availability[ShaderPreset::Off.index()]);
        assert!(update.projection.availability[ShaderPreset::Fsr.index()]);
        assert!(!update.projection.availability[ShaderPreset::ModeA.index()]);
        assert!(update.projection.status.contains("network unavailable"));
    }

    #[test]
    fn stale_acknowledgements_are_ignored() {
        let mut coordinator =
            ShaderCoordinator::with_readiness(ShaderPreset::Fsr, readiness(&[ShaderPreset::Fsr]));
        let stale = coordinator.initial_update().command.expect("initial clear");
        let current = coordinator
            .set_context_capability(ShaderContextCapability::Supported)
            .command
            .expect("current configuration");

        assert!(coordinator.configured(stale.request_id).is_none());
        assert!(coordinator.configured(current.request_id).is_some());
    }

    #[test]
    fn rejected_presets_do_not_retry_until_state_changes() {
        let mut coordinator = ShaderCoordinator::with_readiness(
            ShaderPreset::ModeA,
            readiness(&[ShaderPreset::ModeA]),
        );
        let request = coordinator
            .set_context_capability(ShaderContextCapability::Supported)
            .command
            .expect("configuration request");
        let rejected = coordinator
            .rejected(request.request_id, "shader compilation failed".to_owned())
            .expect("matching rejection");

        assert!(rejected.command.is_none());
        assert!(!rejected.projection.availability[ShaderPreset::ModeA.index()]);
        assert!(
            coordinator
                .set_download_state(false, None)
                .command
                .is_none()
        );
    }

    #[test]
    fn invalid_indices_are_rejected_and_map_to_off_at_boundaries() {
        assert!(ShaderPreset::try_from(-1).is_err());
        assert!(ShaderPreset::try_from(8_i32).is_err());
        assert_eq!(super::preset_from_config(255), ShaderPreset::Off);
        assert_eq!(super::preset_from_ui(99), ShaderPreset::Off);
    }

    #[test]
    fn every_preset_path_has_a_matching_required_file() {
        for preset in ShaderPreset::ALL {
            assert_eq!(preset.paths().len(), preset.required_files().len());
            for (path, file) in preset.paths().iter().zip(preset.required_files()) {
                assert_eq!(path.strip_prefix("~~/shaders/"), Some(*file));
            }
        }
        assert_eq!(ShaderPreset::Fsr.index(), 7);
        assert_eq!(ShaderPreset::Fsr.required_files(), ["FSR.glsl"]);
    }
}
