use std::{cell::RefCell, ffi::c_void, process::Command, rc::Rc, time::Duration};

use slint::{ComponentHandle, Timer, TimerMode};
use thiserror::Error;

use crate::args::{AppArgs, RendererChoice, VideoOutputChoice};

slint::slint! {
    export component RendererProbe inherits Window {
        width: 1px;
        height: 1px;
        background: transparent;
    }
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererProfile {
    SkiaOpenGl,
    FemtoVgOpenGl,
    Software,
}

impl RendererProfile {
    pub fn slint_name(self) -> &'static str {
        match self {
            Self::SkiaOpenGl => "skia-opengl",
            Self::FemtoVgOpenGl => "femtovg",
            Self::Software => "software",
        }
    }

    pub fn uses_open_gl(self) -> bool {
        self != Self::Software
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VideoOutputProfile {
    NativeWindow,
    SharedOpenGl,
    Software,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedRendering {
    pub renderer: RendererProfile,
    pub video_output: VideoOutputProfile,
    pub alpha_bits: Option<i32>,
    pub attempt: usize,
}

#[derive(Clone, Copy, Debug)]
struct AttemptPlan {
    renderer: RendererProfile,
    next_attempt: Option<usize>,
}

pub fn initialize(args: &AppArgs) -> Result<ResolvedRendering, RenderingError> {
    let legacy_backend = std::env::var("SLINT_BACKEND").ok();
    let plan = plan_attempt(args, legacy_backend.as_deref())?;

    tracing::info!(
        renderer = plan.renderer.slint_name(),
        requested_video_output = %args.video_output,
        attempt = args.renderer_attempt,
        legacy_backend = legacy_backend.as_deref(),
        "selecting Slint renderer"
    );

    slint::BackendSelector::new()
        .backend_name("winit".to_owned())
        .renderer_name(plan.renderer.slint_name().to_owned())
        .select()
        .map_err(|source| RenderingError::AttemptFailed {
            renderer: plan.renderer,
            next_attempt: plan.next_attempt,
            detail: source.to_string(),
        })?;

    let probe = probe_renderer(plan.renderer).map_err(|source| RenderingError::AttemptFailed {
        renderer: plan.renderer,
        next_attempt: plan.next_attempt,
        detail: source.to_string(),
    })?;
    let video_output = resolve_video_output(args.video_output, plan.renderer, probe.alpha_bits)
        .map_err(|detail| RenderingError::AttemptFailed {
            renderer: plan.renderer,
            next_attempt: plan.next_attempt,
            detail,
        })?;

    tracing::info!(
        renderer = plan.renderer.slint_name(),
        ?video_output,
        alpha_bits = probe.alpha_bits,
        "rendering architecture profile selected"
    );
    Ok(ResolvedRendering {
        renderer: plan.renderer,
        video_output,
        alpha_bits: probe.alpha_bits,
        attempt: args.renderer_attempt,
    })
}

pub fn relaunch_next(args: &AppArgs, error: &RenderingError) -> Result<bool, RenderingError> {
    let Some(next_attempt) = error.next_attempt() else {
        return Ok(false);
    };
    let executable = std::env::current_exe().map_err(RenderingError::CurrentExecutable)?;
    let arguments = args.relaunch_arguments(next_attempt);
    tracing::warn!(
        %error,
        next_attempt,
        executable = %executable.display(),
        "renderer initialization failed; launching the next process-level fallback"
    );
    Command::new(&executable)
        .args(arguments)
        .spawn()
        .map_err(|source| RenderingError::Relaunch { executable, source })?;
    Ok(true)
}

pub fn reported_setup_failure(
    args: &AppArgs,
    resolved: ResolvedRendering,
    detail: impl Into<String>,
) -> RenderingError {
    let legacy_backend = std::env::var("SLINT_BACKEND").ok();
    let next_attempt = plan_attempt(args, legacy_backend.as_deref())
        .ok()
        .and_then(|plan| plan.next_attempt);
    RenderingError::AttemptFailed {
        renderer: resolved.renderer,
        next_attempt,
        detail: detail.into(),
    }
}

pub fn show_terminal_error(error: &RenderingError) {
    let message = format!(
        "Stremio could not initialize a compatible renderer.\n\n{error}\n\nDetailed attempts are recorded in storage\\logs\\stremio.log."
    );
    tracing::error!(%error, "all renderer attempts failed");
    show_error_dialog("Stremio renderer error", &message);
}

fn plan_attempt(
    args: &AppArgs,
    legacy_backend: Option<&str>,
) -> Result<AttemptPlan, RenderingError> {
    let requested = args.effective_renderer(legacy_backend);
    let forced = args.renderer_is_forced(legacy_backend);
    let needs_open_gl = matches!(
        args.video_output,
        VideoOutputChoice::NativeWindow | VideoOutputChoice::SharedOpenGl
    );

    let candidates = if forced {
        match requested {
            RendererChoice::SkiaOpenGl => vec![RendererProfile::SkiaOpenGl],
            RendererChoice::FemtoVg => vec![RendererProfile::FemtoVgOpenGl],
            RendererChoice::Software if needs_open_gl => {
                return Err(RenderingError::InvalidCombination(
                    "the software UI renderer cannot host native-window or shared-opengl video"
                        .to_owned(),
                ));
            }
            RendererChoice::Software => vec![RendererProfile::Software],
            RendererChoice::Auto => {
                return Err(RenderingError::InvalidCombination(
                    "an automatic renderer was unexpectedly marked as forced".to_owned(),
                ));
            }
        }
    } else {
        let mut candidates = vec![RendererProfile::SkiaOpenGl, RendererProfile::FemtoVgOpenGl];
        if !needs_open_gl {
            candidates.push(RendererProfile::Software);
        }
        candidates
    };

    let renderer =
        candidates
            .get(args.renderer_attempt)
            .copied()
            .ok_or(RenderingError::NoCandidate {
                attempt: args.renderer_attempt,
            })?;
    let next_attempt = (!forced && args.renderer_attempt + 1 < candidates.len())
        .then_some(args.renderer_attempt + 1);
    Ok(AttemptPlan {
        renderer,
        next_attempt,
    })
}

fn resolve_video_output(
    requested: VideoOutputChoice,
    renderer: RendererProfile,
    alpha_bits: Option<i32>,
) -> Result<VideoOutputProfile, String> {
    if renderer == RendererProfile::Software {
        return match requested {
            VideoOutputChoice::Auto | VideoOutputChoice::Software => {
                Ok(VideoOutputProfile::Software)
            }
            VideoOutputChoice::NativeWindow | VideoOutputChoice::SharedOpenGl => Err(format!(
                "{} video requires an OpenGL Slint renderer",
                requested
            )),
        };
    }

    match requested {
        VideoOutputChoice::Software => Ok(VideoOutputProfile::Software),
        VideoOutputChoice::SharedOpenGl => Ok(VideoOutputProfile::SharedOpenGl),
        VideoOutputChoice::NativeWindow => {
            #[cfg(not(windows))]
            return Err("native-window video is currently supported only on Windows".to_owned());
            #[cfg(windows)]
            if alpha_bits.unwrap_or_default() <= 0 {
                Err(
                    "native-window video requires an OpenGL surface with an alpha channel"
                        .to_owned(),
                )
            } else {
                Ok(VideoOutputProfile::NativeWindow)
            }
        }
        VideoOutputChoice::Auto => {
            #[cfg(windows)]
            if alpha_bits.unwrap_or_default() > 0 {
                return Ok(VideoOutputProfile::NativeWindow);
            }
            Ok(VideoOutputProfile::SharedOpenGl)
        }
    }
}

#[derive(Default)]
struct ProbeState {
    ready: bool,
    timed_out: bool,
    alpha_bits: Option<i32>,
    error: Option<String>,
}

struct ProbeResult {
    alpha_bits: Option<i32>,
}

fn probe_renderer(renderer: RendererProfile) -> Result<ProbeResult, RendererProbeError> {
    let probe = RendererProbe::new().map_err(RendererProbeError::CreateWindow)?;
    let state = Rc::new(RefCell::new(ProbeState::default()));
    let timeout = Rc::new(Timer::default());

    if renderer.uses_open_gl() {
        let state_for_notifier = state.clone();
        let timeout_for_notifier = timeout.clone();
        probe
            .window()
            .set_rendering_notifier(move |rendering_state, graphics_api| {
                if !matches!(rendering_state, slint::RenderingState::RenderingSetup) {
                    return;
                }
                timeout_for_notifier.stop();
                let mut state = state_for_notifier.borrow_mut();
                match graphics_api {
                    slint::GraphicsAPI::NativeOpenGL { get_proc_address } => {
                        match open_gl_alpha_bits(get_proc_address) {
                            Ok(alpha_bits) => {
                                state.ready = true;
                                state.alpha_bits = Some(alpha_bits);
                            }
                            Err(error) => state.error = Some(error),
                        }
                    }
                    other => {
                        state.error = Some(format!(
                            "renderer exposed {other:?} instead of NativeOpenGL"
                        ));
                    }
                }
                drop(state);
                let _ = slint::quit_event_loop();
            })
            .map_err(RendererProbeError::Notifier)?;
    } else {
        use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

        let state_for_event = state.clone();
        let timeout_for_event = timeout.clone();
        probe.window().on_winit_window_event(move |_window, event| {
            if matches!(event, winit::event::WindowEvent::RedrawRequested) {
                timeout_for_event.stop();
                state_for_event.borrow_mut().ready = true;
                let _ = slint::quit_event_loop();
            }
            EventResult::Propagate
        });
    }

    let state_for_timeout = state.clone();
    let weak_probe = probe.as_weak();
    timeout.start(TimerMode::SingleShot, PROBE_TIMEOUT, move || {
        state_for_timeout.borrow_mut().timed_out = true;
        if let Some(probe) = weak_probe.upgrade() {
            let _ = probe.hide();
        }
        let _ = slint::quit_event_loop();
    });

    probe.show().map_err(RendererProbeError::ShowWindow)?;
    slint::run_event_loop().map_err(RendererProbeError::EventLoop)?;
    timeout.stop();
    let _ = probe.hide();

    let state = state.borrow();
    if state.timed_out {
        return Err(RendererProbeError::Timeout(PROBE_TIMEOUT));
    }
    if let Some(error) = state.error.as_ref() {
        return Err(RendererProbeError::Graphics(error.clone()));
    }
    if !state.ready {
        return Err(RendererProbeError::Graphics(
            "the renderer probe ended before presenting a frame".to_owned(),
        ));
    }
    Ok(ProbeResult {
        alpha_bits: state.alpha_bits,
    })
}

pub(crate) fn open_gl_alpha_bits(
    get_proc_address: &dyn Fn(&std::ffi::CStr) -> *const c_void,
) -> Result<i32, String> {
    let address = get_proc_address(c"glGetIntegerv");
    if address.is_null() {
        return Err("OpenGL did not expose glGetIntegerv".to_owned());
    }
    type GetInteger = unsafe extern "system" fn(u32, *mut i32);
    // SAFETY: The active OpenGL context returned a pointer for the exact
    // glGetIntegerv ABI and keeps it valid for the duration of this call.
    let get_integer: GetInteger = unsafe { std::mem::transmute(address) };
    let mut alpha_bits = 0;
    // SAFETY: The output points to a live i32 and GL_ALPHA_BITS is valid here.
    unsafe { get_integer(glow::ALPHA_BITS, &mut alpha_bits) };
    Ok(alpha_bits)
}

#[derive(Debug, Error)]
pub enum RenderingError {
    #[error("renderer {renderer:?} failed: {detail}")]
    AttemptFailed {
        renderer: RendererProfile,
        next_attempt: Option<usize>,
        detail: String,
    },
    #[error("renderer attempt {attempt} has no matching candidate")]
    NoCandidate { attempt: usize },
    #[error("invalid rendering configuration: {0}")]
    InvalidCombination(String),
    #[error("could not locate the current executable: {0}")]
    CurrentExecutable(std::io::Error),
    #[error("could not relaunch {executable}: {source}")]
    Relaunch {
        executable: std::path::PathBuf,
        source: std::io::Error,
    },
}

impl RenderingError {
    fn next_attempt(&self) -> Option<usize> {
        match self {
            Self::AttemptFailed { next_attempt, .. } => *next_attempt,
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
enum RendererProbeError {
    #[error("could not create renderer probe: {0}")]
    CreateWindow(slint::PlatformError),
    #[error("could not install renderer probe notifier: {0}")]
    Notifier(slint::SetRenderingNotifierError),
    #[error("could not show renderer probe: {0}")]
    ShowWindow(slint::PlatformError),
    #[error("renderer probe event loop failed: {0}")]
    EventLoop(slint::PlatformError),
    #[error("renderer probe timed out after {0:?}")]
    Timeout(Duration),
    #[error("renderer probe failed: {0}")]
    Graphics(String),
}

#[cfg(windows)]
fn show_error_dialog(title: &str, message: &str) {
    use windows::{
        Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
        core::PCWSTR,
    };

    let title = title.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let message = message.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    // SAFETY: Both UTF-16 buffers are null terminated and remain alive for the
    // synchronous MessageBoxW call.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_error_dialog(title: &str, message: &str) {
    eprintln!("{title}: {message}");
}

#[cfg(test)]
mod tests {
    use super::{RendererProfile, VideoOutputProfile, plan_attempt, resolve_video_output};
    use crate::args::AppArgs;
    use std::ffi::OsString;

    fn args(arguments: &[&str]) -> AppArgs {
        AppArgs::parse(arguments.iter().map(OsString::from)).expect("arguments should parse")
    }

    #[test]
    fn auto_renderer_falls_back_in_the_expected_order() {
        let args = args(&["app", "--renderer-attempt=1"]);
        assert_eq!(
            plan_attempt(&args, None).expect("second attempt").renderer,
            RendererProfile::FemtoVgOpenGl
        );
    }

    #[test]
    fn shared_gl_excludes_the_software_ui_candidate() {
        let args = args(&[
            "app",
            "--video-output=shared-opengl",
            "--renderer-attempt=2",
        ]);
        assert!(plan_attempt(&args, None).is_err());
    }

    #[test]
    fn software_ui_selects_software_video_for_auto() {
        assert_eq!(
            resolve_video_output(
                crate::args::VideoOutputChoice::Auto,
                RendererProfile::Software,
                None,
            ),
            Ok(VideoOutputProfile::Software)
        );
    }

    #[cfg(windows)]
    #[test]
    fn transparent_gl_selects_native_video_for_auto() {
        assert_eq!(
            resolve_video_output(
                crate::args::VideoOutputChoice::Auto,
                RendererProfile::SkiaOpenGl,
                Some(8),
            ),
            Ok(VideoOutputProfile::NativeWindow)
        );
    }
}
