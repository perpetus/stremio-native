//! Native MPV playback and render integration primitives.
//!
//! The crate links the MPV import library pinned by `mpv.lock.json`, validates
//! the deployed DLL's client API, serializes player commands on one actor
//! thread, and exposes native-window, OpenGL, or bounded software video output.

#![deny(unsafe_op_in_unsafe_fn)]

mod actor;
mod ffi;
mod render;
mod software;

pub use actor::{
    AudioTrack, EndReason, NativeWindowTarget, PlaybackCommand, PlaybackController, PlaybackEvent,
    PlaybackRuntime, PlaybackState, PlaybackVideoSource, PlayerConfig, PlayerVideoOutput,
    SubtitleTrack,
};
pub use ffi::{ApiVersion, HEADER_CLIENT_API_VERSION, MpvError};
pub use render::{
    OpenGlDiagnostics, OpenGlProcAddress, RenderContext as OpenGlRenderContext,
    RenderOutcome as OpenGlRenderOutcome, RenderSource as OpenGlRenderSource,
    VideoTexture as OpenGlVideoTexture,
};
pub use software::{SoftwareFrame, SoftwareFrameSource, SoftwareRenderConfig};
