//! Native MPV playback and render integration primitives.
//!
//! The crate statically links the MPV SDK pinned by `mpv.lock.json`, validates
//! its client API, serializes player commands on one actor thread, and exposes
//! an OpenGL render context for the GUI thread.

#![deny(unsafe_op_in_unsafe_fn)]

mod actor;
mod ffi;
mod render;

pub use actor::{
    AudioTrack, EndReason, PlaybackCommand, PlaybackController, PlaybackEvent, PlaybackRuntime,
    PlaybackState, PlayerConfig, SubtitleTrack,
};
pub use ffi::{ApiVersion, HEADER_CLIENT_API_VERSION, MpvError};
pub use render::{
    OpenGlDiagnostics, OpenGlProcAddress, RenderContext, RenderOutcome, RenderSource, VideoTexture,
};
