use std::{ffi::OsString, fmt};

use thiserror::Error;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RendererChoice {
    #[default]
    Auto,
    SkiaOpenGl,
    FemtoVg,
    Software,
}

impl RendererChoice {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "skia-opengl" => Some(Self::SkiaOpenGl),
            "femtovg" => Some(Self::FemtoVg),
            "software" => Some(Self::Software),
            _ => None,
        }
    }

    pub fn from_legacy_slint_backend(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "winit-skia-opengl" | "skia-opengl" => Some(Self::SkiaOpenGl),
            "winit-femtovg" | "femtovg" => Some(Self::FemtoVg),
            "winit-software" | "software" => Some(Self::Software),
            _ => None,
        }
    }
}

impl fmt::Display for RendererChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::SkiaOpenGl => "skia-opengl",
            Self::FemtoVg => "femtovg",
            Self::Software => "software",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VideoOutputChoice {
    #[default]
    Auto,
    NativeWindow,
    SharedOpenGl,
    Software,
}

impl VideoOutputChoice {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "native-window" => Some(Self::NativeWindow),
            "shared-opengl" => Some(Self::SharedOpenGl),
            "software" => Some(Self::Software),
            _ => None,
        }
    }
}

impl fmt::Display for VideoOutputChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::NativeWindow => "native-window",
            Self::SharedOpenGl => "shared-opengl",
            Self::Software => "software",
        })
    }
}

#[derive(Clone, Debug)]
pub struct AppArgs {
    renderer: RendererChoice,
    renderer_from_cli: bool,
    pub video_output: VideoOutputChoice,
    pub renderer_attempt: usize,
    original: Vec<OsString>,
}

impl AppArgs {
    pub fn from_env() -> Result<Self, ArgsError> {
        Self::parse(std::env::args_os())
    }

    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, ArgsError> {
        let original = args.into_iter().skip(1).collect::<Vec<_>>();
        let mut renderer = RendererChoice::Auto;
        let mut renderer_from_cli = false;
        let mut video_output = VideoOutputChoice::Auto;
        let mut renderer_attempt = 0;
        let mut index = 0;

        while index < original.len() {
            let argument = original[index].to_string_lossy();
            if let Some(value) = argument.strip_prefix("--renderer=") {
                renderer = RendererChoice::parse(value).ok_or_else(|| ArgsError::InvalidValue {
                    option: "--renderer",
                    value: value.to_owned(),
                })?;
                renderer_from_cli = true;
            } else if argument == "--renderer" {
                let value = value_after(&original, index, "--renderer")?;
                renderer = RendererChoice::parse(value).ok_or_else(|| ArgsError::InvalidValue {
                    option: "--renderer",
                    value: value.to_owned(),
                })?;
                renderer_from_cli = true;
                index += 1;
            } else if let Some(value) = argument.strip_prefix("--video-output=") {
                video_output =
                    VideoOutputChoice::parse(value).ok_or_else(|| ArgsError::InvalidValue {
                        option: "--video-output",
                        value: value.to_owned(),
                    })?;
            } else if argument == "--video-output" {
                let value = value_after(&original, index, "--video-output")?;
                video_output =
                    VideoOutputChoice::parse(value).ok_or_else(|| ArgsError::InvalidValue {
                        option: "--video-output",
                        value: value.to_owned(),
                    })?;
                index += 1;
            } else if let Some(value) = argument.strip_prefix("--renderer-attempt=") {
                renderer_attempt = parse_attempt(value)?;
            } else if argument == "--renderer-attempt" {
                let value = value_after(&original, index, "--renderer-attempt")?;
                renderer_attempt = parse_attempt(value)?;
                index += 1;
            }
            index += 1;
        }

        Ok(Self {
            renderer,
            renderer_from_cli,
            video_output,
            renderer_attempt,
            original,
        })
    }

    pub fn effective_renderer(&self, legacy_backend: Option<&str>) -> RendererChoice {
        if self.renderer_from_cli {
            return self.renderer;
        }
        legacy_backend
            .and_then(RendererChoice::from_legacy_slint_backend)
            .unwrap_or(self.renderer)
    }

    pub fn renderer_is_forced(&self, legacy_backend: Option<&str>) -> bool {
        self.renderer_from_cli && self.renderer != RendererChoice::Auto
            || !self.renderer_from_cli
                && legacy_backend
                    .and_then(RendererChoice::from_legacy_slint_backend)
                    .is_some()
    }

    pub fn relaunch_arguments(&self, next_attempt: usize) -> Vec<OsString> {
        let mut arguments = Vec::with_capacity(self.original.len() + 1);
        let mut skip_next = false;
        for argument in &self.original {
            if skip_next {
                skip_next = false;
                continue;
            }
            let text = argument.to_string_lossy();
            if text == "--renderer-attempt" {
                skip_next = true;
                continue;
            }
            if text.starts_with("--renderer-attempt=") {
                continue;
            }
            arguments.push(argument.clone());
        }
        arguments.push(format!("--renderer-attempt={next_attempt}").into());
        arguments
    }
}

fn value_after<'a>(
    arguments: &'a [OsString],
    index: usize,
    option: &'static str,
) -> Result<&'a str, ArgsError> {
    arguments
        .get(index + 1)
        .and_then(|value| value.to_str())
        .ok_or(ArgsError::MissingValue(option))
}

fn parse_attempt(value: &str) -> Result<usize, ArgsError> {
    value
        .parse()
        .map_err(|_| ArgsError::InvalidRendererAttempt(value.to_owned()))
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ArgsError {
    #[error("{0} requires a value")]
    MissingValue(&'static str),
    #[error("invalid value '{value}' for {option}")]
    InvalidValue { option: &'static str, value: String },
    #[error("invalid renderer attempt '{0}'")]
    InvalidRendererAttempt(String),
}

#[cfg(test)]
mod tests {
    use super::{AppArgs, RendererChoice, VideoOutputChoice};
    use std::ffi::OsString;

    fn parse(arguments: &[&str]) -> AppArgs {
        AppArgs::parse(arguments.iter().map(OsString::from)).expect("arguments should parse")
    }

    #[test]
    fn cli_renderer_takes_precedence_over_legacy_backend() {
        let args = parse(&["app", "--renderer=femtovg"]);
        assert_eq!(
            args.effective_renderer(Some("winit-skia-opengl")),
            RendererChoice::FemtoVg
        );
    }

    #[test]
    fn recognized_legacy_backend_is_used_when_cli_is_absent() {
        let args = parse(&["app"]);
        assert_eq!(
            args.effective_renderer(Some("winit-software")),
            RendererChoice::Software
        );
        assert!(args.renderer_is_forced(Some("winit-software")));
    }

    #[test]
    fn renderer_and_video_output_accept_split_values() {
        let args = parse(&[
            "app",
            "--renderer",
            "skia-opengl",
            "--video-output",
            "software",
        ]);
        assert_eq!(args.effective_renderer(None), RendererChoice::SkiaOpenGl);
        assert_eq!(args.video_output, VideoOutputChoice::Software);
    }

    #[test]
    fn relaunch_replaces_hidden_attempt_without_touching_other_arguments() {
        let args = parse(&[
            "app",
            "--profile=ui",
            "--renderer-attempt",
            "1",
            "--video-output=auto",
        ]);
        assert_eq!(
            args.relaunch_arguments(2),
            vec![
                OsString::from("--profile=ui"),
                OsString::from("--video-output=auto"),
                OsString::from("--renderer-attempt=2"),
            ]
        );
    }
}
