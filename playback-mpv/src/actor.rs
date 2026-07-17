use std::{
    ffi::{CStr, c_char, c_int, c_void},
    num::NonZeroU32,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    ffi::{
        END_FILE_EOF, END_FILE_ERROR, END_FILE_QUIT, END_FILE_REDIRECT, END_FILE_STOP,
        EVENT_END_FILE, EVENT_FILE_LOADED, EVENT_NONE, EVENT_PLAYBACK_RESTART,
        EVENT_PROPERTY_CHANGE, EVENT_QUEUE_OVERFLOW, EVENT_SHUTDOWN, EVENT_START_FILE,
        FORMAT_DOUBLE, FORMAT_FLAG, FORMAT_INT64, FORMAT_NODE, FORMAT_NODE_ARRAY, FORMAT_NODE_MAP,
        FORMAT_NONE, FORMAT_STRING, MpvApi, MpvClient, MpvError, MpvEvent, MpvEventEndFile,
        MpvEventProperty, MpvNode, MpvNodeList,
    },
    render::RenderSource,
    software::{SoftwareFrameSource, SoftwareRenderConfig, SoftwareRenderRuntime},
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioTrack {
    pub id: String,
    pub title: Option<String>,
    pub language: Option<String>,
    pub codec: Option<String>,
    pub selected: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubtitleTrack {
    pub id: String,
    pub title: Option<String>,
    pub language: Option<String>,
    pub codec: Option<String>,
    pub selected: bool,
    pub external: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackState {
    pub loading: bool,
    pub loaded: bool,
    pub paused: bool,
    pub buffering: bool,
    pub seeking: bool,
    pub time: f64,
    pub duration: f64,
    pub buffered_until: f64,
    pub cache_buffering_percent: f64,
    pub volume: f64,
    pub muted: bool,
    pub speed: f64,
    pub audio_tracks: Vec<AudioTrack>,
    pub subtitle_tracks: Vec<SubtitleTrack>,
    pub active_audio_track: Option<String>,
    pub active_subtitle_track: Option<String>,
    pub filename: Option<String>,
    pub file_size: Option<u64>,
    pub file_format: Option<String>,
    pub video_format: Option<String>,
    pub audio_format: Option<String>,
    pub hardware_decoder: Option<String>,
    pub has_video_track: bool,
    pub vo_configured: bool,
    pub current_vo: Option<String>,
    pub current_gpu_context: Option<String>,
    pub current_hwdec: Option<String>,
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self {
            loading: false,
            loaded: false,
            paused: true,
            buffering: false,
            seeking: false,
            time: 0.0,
            duration: 0.0,
            buffered_until: 0.0,
            cache_buffering_percent: 0.0,
            volume: 1.0,
            muted: false,
            speed: 1.0,
            audio_tracks: Vec::new(),
            subtitle_tracks: Vec::new(),
            active_audio_track: None,
            active_subtitle_track: None,
            filename: None,
            file_size: None,
            file_format: None,
            video_format: None,
            audio_format: None,
            hardware_decoder: None,
            has_video_track: false,
            vo_configured: false,
            current_vo: None,
            current_gpu_context: None,
            current_hwdec: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndReason {
    Eof,
    Stopped,
    Quit,
    Error,
    Redirect,
    Unknown,
}

#[derive(Clone, Debug)]
pub enum PlaybackEvent {
    State(Box<PlaybackState>),
    FileLoaded,
    PlaybackRestarted,
    Ended {
        reason: EndReason,
        error_code: Option<i32>,
        error: Option<String>,
    },
    Warning(String),
    Error(String),
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum PlaybackCommand {
    Load { url: String, start_at: Option<f64> },
    Stop,
    SetPaused(bool),
    TogglePaused,
    SeekAbsolute(f64),
    SeekRelative(f64),
    SetVolume(f64),
    SetMuted(bool),
    SetSpeed(f64),
    SetVideoScale(u8),
    SetAudioTrack(Option<String>),
    SetSubtitleTrack(Option<String>),
    AddSubtitle { url: String, title: Option<String> },
    SetSubtitleDelay(i64),
    SetSubtitleScale(f64),
    SetSubtitlePosition(f64),
    SetAudioDelay(i64),
    Shutdown,
}

#[derive(Clone, Debug)]
pub struct PlayerConfig {
    pub config_dir: Option<PathBuf>,
    pub hardware_decoding: bool,
    pub video_output: PlayerVideoOutput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum NativeWindowTarget {
    Win32(NonZeroU32),
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum PlayerVideoOutput {
    NativeWindow(NativeWindowTarget),
    OpenGlRenderApi,
    SoftwareRenderApi(SoftwareRenderConfig),
}

#[derive(Clone)]
#[non_exhaustive]
pub enum PlaybackVideoSource {
    NativeWindow,
    OpenGl(RenderSource),
    Software(SoftwareFrameSource),
}

#[derive(Clone)]
pub struct PlaybackController {
    sender: SyncSender<PlaybackCommand>,
    wake: Arc<ActorWake>,
}

impl PlaybackController {
    pub fn send(&self, command: PlaybackCommand) -> Result<(), MpvError> {
        self.sender
            .try_send(command)
            .map(|()| self.wake.signal())
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => MpvError::CommandQueueFull,
                mpsc::TrySendError::Disconnected(_) => MpvError::CommandQueueClosed,
            })
    }

    fn shutdown(&self) {
        // Shutdown must not be dropped just because the bounded queue is
        // temporarily full. The actor continuously drains this queue.
        self.wake.signal();
        let _ = self.sender.send(PlaybackCommand::Shutdown);
        self.wake.signal();
    }
}

pub struct PlaybackRuntime {
    controller: PlaybackController,
    video_source: PlaybackVideoSource,
    software_render: Option<SoftwareRenderRuntime>,
    actor: Option<JoinHandle<()>>,
}

impl PlaybackRuntime {
    pub fn start(
        config: PlayerConfig,
        event_sink: impl Fn(PlaybackEvent) + Send + Sync + 'static,
    ) -> Result<Self, MpvError> {
        let api = MpvApi::linked()?;
        let client = MpvClient::create(api)?;

        if let Some(config_dir) = config.config_dir {
            client.set_option("config", "yes")?;
            client.set_option("load-scripts", "yes")?;
            client.set_option("config-dir", &config_dir.to_string_lossy())?;
        }
        client.set_option("terminal", "no")?;
        client.set_option("input-default-bindings", "no")?;
        client.set_option("input-vo-keyboard", "no")?;
        client.set_option("osc", "no")?;
        client.set_option("idle", "yes")?;
        client.set_option("keep-open", "no")?;
        client.set_option("cache", "no")?;
        client.set_option("cache-pause", "yes")?;
        client.set_option("cache-pause-initial", "no")?;
        client.set_option("cache-pause-wait", "0.5")?;
        client.set_option("cache-secs", "60")?;
        client.set_option("demuxer-max-bytes", "300000000")?;
        client.set_option("vd-lavc-threads", "0")?;
        client.set_option("ad-lavc-threads", "0")?;
        client.set_option("audio-fallback-to-null", "yes")?;
        client.set_option("audio-client-name", "Stremio")?;
        client.set_option("title", "Stremio")?;
        configure_video_output(&client, &config.video_output, config.hardware_decoding)?;
        client.initialize()?;
        observe_properties(&client)?;

        let (video_source, software_render) = match config.video_output {
            PlayerVideoOutput::NativeWindow(_) => (PlaybackVideoSource::NativeWindow, None),
            PlayerVideoOutput::OpenGlRenderApi => (
                PlaybackVideoSource::OpenGl(RenderSource::new(client.clone())),
                None,
            ),
            PlayerVideoOutput::SoftwareRenderApi(config) => {
                let runtime = SoftwareRenderRuntime::start(client.clone(), config)?;
                let source = runtime.source();
                (PlaybackVideoSource::Software(source), Some(runtime))
            }
        };

        let (sender, receiver) = mpsc::sync_channel(128);
        let wake = Arc::new(ActorWake::default());
        client.set_wakeup_callback(
            Some(wakeup_actor),
            Arc::as_ptr(&wake).cast_mut().cast::<c_void>(),
        );
        let controller = PlaybackController {
            sender,
            wake: wake.clone(),
        };
        let sink = Arc::new(event_sink);
        wake.signal();
        let actor = thread::Builder::new()
            .name("mpv-player".to_owned())
            .spawn(move || actor_loop(client, receiver, sink, wake))
            .map_err(|_| MpvError::ActorPanicked)?;

        Ok(Self {
            controller,
            video_source,
            software_render,
            actor: Some(actor),
        })
    }

    pub fn controller(&self) -> PlaybackController {
        self.controller.clone()
    }

    pub fn video_source(&self) -> PlaybackVideoSource {
        self.video_source.clone()
    }

    pub fn shutdown(mut self) -> Result<(), MpvError> {
        if let Some(software_render) = self.software_render.take() {
            software_render.shutdown()?;
        }
        self.controller.shutdown();
        if let Some(actor) = self.actor.take() {
            actor.join().map_err(|_| MpvError::ActorPanicked)?;
        }
        Ok(())
    }
}

fn configure_video_output(
    client: &MpvClient,
    output: &PlayerVideoOutput,
    hardware_decoding: bool,
) -> Result<(), MpvError> {
    for (name, value) in video_options(output, hardware_decoding) {
        client.set_option(name, &value)?;
    }
    Ok(())
}

fn video_options(
    output: &PlayerVideoOutput,
    hardware_decoding: bool,
) -> Vec<(&'static str, String)> {
    match output {
        PlayerVideoOutput::NativeWindow(NativeWindowTarget::Win32(hwnd)) => vec![
            ("wid", hwnd.get().to_string()),
            ("vo", "gpu-next,gpu,direct3d".to_owned()),
            ("gpu-context", "d3d11".to_owned()),
            ("gpu-api", "d3d11".to_owned()),
            ("d3d11-warp", "auto".to_owned()),
            ("vd-lavc-dr", "auto".to_owned()),
            (
                "hwdec",
                if hardware_decoding {
                    "d3d11va,auto-safe"
                } else {
                    "no"
                }
                .to_owned(),
            ),
        ],
        PlayerVideoOutput::OpenGlRenderApi => vec![
            ("vo", "libmpv".to_owned()),
            ("vd-lavc-dr", "no".to_owned()),
            (
                "hwdec",
                if hardware_decoding {
                    copy_safe_hardware_decoding()
                } else {
                    "no"
                }
                .to_owned(),
            ),
        ],
        PlayerVideoOutput::SoftwareRenderApi(_) => vec![
            ("vo", "libmpv".to_owned()),
            ("profile", "sw-fast".to_owned()),
            ("vd-lavc-dr", "no".to_owned()),
            ("hwdec", "no".to_owned()),
        ],
    }
}

fn copy_safe_hardware_decoding() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "d3d11va-copy,auto-copy"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "auto-copy"
    }
}

impl Drop for PlaybackRuntime {
    fn drop(&mut self) {
        if let Some(software_render) = self.software_render.take() {
            let _ = software_render.shutdown();
        }
        if let Some(actor) = self.actor.take() {
            self.controller.shutdown();
            let _ = actor.join();
        }
    }
}

fn observe_properties(client: &MpvClient) -> Result<(), MpvError> {
    let properties = [
        (1, "pause", FORMAT_FLAG),
        (2, "time-pos", FORMAT_DOUBLE),
        (3, "duration", FORMAT_DOUBLE),
        (4, "demuxer-cache-time", FORMAT_DOUBLE),
        (5, "paused-for-cache", FORMAT_FLAG),
        (6, "seeking", FORMAT_FLAG),
        (7, "volume", FORMAT_DOUBLE),
        (8, "mute", FORMAT_FLAG),
        (9, "speed", FORMAT_DOUBLE),
        (10, "aid", FORMAT_STRING),
        (11, "sid", FORMAT_STRING),
        (12, "track-list", FORMAT_NODE),
        (13, "filename", FORMAT_STRING),
        (14, "file-size", FORMAT_INT64),
        (15, "file-format", FORMAT_STRING),
        (16, "video-format", FORMAT_STRING),
        (17, "audio-codec-name", FORMAT_STRING),
        (18, "hwdec-current", FORMAT_STRING),
        (19, "cache-buffering-state", FORMAT_INT64),
        (20, "vo-configured", FORMAT_FLAG),
        (21, "current-vo", FORMAT_STRING),
        (22, "gpu-context", FORMAT_STRING),
    ];
    for (id, name, format) in properties {
        client.observe(id, name, format)?;
    }
    Ok(())
}

fn actor_loop(
    client: Arc<MpvClient>,
    receiver: Receiver<PlaybackCommand>,
    sink: Arc<dyn Fn(PlaybackEvent) + Send + Sync>,
    wake: Arc<ActorWake>,
) {
    let mut state = PlaybackState::default();
    let mut running = true;

    while running {
        wake.wait(Duration::from_secs(1));
        loop {
            match receiver.try_recv() {
                Ok(command) => {
                    running = handle_command(&client, command, &mut state, &sink);
                    if !running {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    running = false;
                    break;
                }
            }
        }
        if !running {
            break;
        }

        drain_events(&client, &mut state, &sink, &mut running);
    }

    client.set_wakeup_callback(None, std::ptr::null_mut());
    let _ = client.command(&["stop"]);
    sink(PlaybackEvent::Shutdown);
}

#[derive(Default)]
struct ActorWake {
    pending: Mutex<bool>,
    condvar: Condvar,
}

impl ActorWake {
    fn signal(&self) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *pending = true;
        self.condvar.notify_one();
    }

    fn wait(&self, timeout: Duration) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !*pending {
            let (guard, _) = self
                .condvar
                .wait_timeout(pending, timeout)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            pending = guard;
        }
        *pending = false;
    }
}

unsafe extern "C" fn wakeup_actor(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    // SAFETY: The pointer comes from Arc::as_ptr and remains alive until the
    // callback is unregistered at actor shutdown.
    let wake = unsafe { &*context.cast::<ActorWake>() };
    wake.signal();
}

fn handle_command(
    client: &MpvClient,
    command: PlaybackCommand,
    state: &mut PlaybackState,
    sink: &Arc<dyn Fn(PlaybackEvent) + Send + Sync>,
) -> bool {
    let fatal_error = matches!(&command, PlaybackCommand::Load { .. });
    let result = match command {
        PlaybackCommand::Load { url, start_at } => {
            *state = PlaybackState {
                loading: true,
                paused: false,
                ..PlaybackState::default()
            };
            sink(PlaybackEvent::State(Box::new(state.clone())));
            match start_at.filter(|time| time.is_finite() && *time > 0.0) {
                Some(start_at) => {
                    let options = format!("start={start_at:.3}");
                    client.command(&["loadfile", &url, "replace", "-1", &options])
                }
                None => client.command(&["loadfile", &url, "replace"]),
            }
        }
        PlaybackCommand::Stop => client.command(&["stop"]),
        PlaybackCommand::SetPaused(paused) => client.set_flag("pause", paused),
        PlaybackCommand::TogglePaused => client.command(&["cycle", "pause"]),
        PlaybackCommand::SeekAbsolute(time) => {
            client.command(&["seek", &time.max(0.0).to_string(), "absolute+exact"])
        }
        PlaybackCommand::SeekRelative(seconds) => {
            client.command(&["seek", &seconds.to_string(), "relative+exact"])
        }
        PlaybackCommand::SetVolume(volume) => {
            client.set_double("volume", volume.clamp(0.0, 1.0) * 100.0)
        }
        PlaybackCommand::SetMuted(muted) => client.set_flag("mute", muted),
        PlaybackCommand::SetSpeed(speed) => client.set_double("speed", speed.clamp(0.25, 4.0)),
        PlaybackCommand::SetVideoScale(mode) => match mode % 3 {
            // contain: preserve the source aspect and letterbox inside the FBO
            0 => client
                .set_flag("keepaspect", true)
                .and_then(|()| client.set_double("panscan", 0.0)),
            // cover: preserve the source aspect and crop until the FBO is full
            1 => client
                .set_flag("keepaspect", true)
                .and_then(|()| client.set_double("panscan", 1.0)),
            // fill: match the FBO exactly (the web player's third scale mode)
            _ => client.set_flag("keepaspect", false),
        },
        PlaybackCommand::SetAudioTrack(track) => {
            client.set_string("aid", track.as_deref().unwrap_or("no"))
        }
        PlaybackCommand::SetSubtitleTrack(track) => {
            client.set_string("sid", track.as_deref().unwrap_or("no"))
        }
        PlaybackCommand::AddSubtitle { url, title } => {
            let title = title.unwrap_or_default();
            client.command(&["sub-add", &url, "auto", &title])
        }
        PlaybackCommand::SetSubtitleDelay(milliseconds) => {
            client.set_double("sub-delay", milliseconds as f64 / 1_000.0)
        }
        PlaybackCommand::SetSubtitleScale(scale) => {
            client.set_double("sub-scale", scale.clamp(0.25, 4.0))
        }
        PlaybackCommand::SetSubtitlePosition(position) => {
            client.set_double("sub-pos", position.clamp(0.0, 100.0))
        }
        PlaybackCommand::SetAudioDelay(milliseconds) => {
            client.set_double("audio-delay", milliseconds as f64 / 1_000.0)
        }
        PlaybackCommand::Shutdown => return false,
    };
    if let Err(error) = result {
        sink(if fatal_error {
            PlaybackEvent::Error(error.to_string())
        } else {
            PlaybackEvent::Warning(error.to_string())
        });
    }
    true
}

fn drain_events(
    client: &MpvClient,
    state: &mut PlaybackState,
    sink: &Arc<dyn Fn(PlaybackEvent) + Send + Sync>,
    running: &mut bool,
) {
    loop {
        let event = client.wait_event(0.0);
        if event.is_null() {
            return;
        }
        // SAFETY: MPV guarantees the event pointer until the next wait call.
        let event = unsafe { &*event };
        match event.event_id {
            EVENT_NONE => return,
            EVENT_START_FILE => {
                state.loading = true;
                state.loaded = false;
                state.cache_buffering_percent = 0.0;
                sink(PlaybackEvent::State(Box::new(state.clone())));
            }
            EVENT_FILE_LOADED => {
                state.loading = false;
                state.loaded = true;
                sink(PlaybackEvent::FileLoaded);
                sink(PlaybackEvent::State(Box::new(state.clone())));
            }
            EVENT_PLAYBACK_RESTART => {
                state.buffering = false;
                sink(PlaybackEvent::State(Box::new(state.clone())));
                sink(PlaybackEvent::PlaybackRestarted);
            }
            EVENT_PROPERTY_CHANGE => {
                update_property(event, state);
                sink(PlaybackEvent::State(Box::new(state.clone())));
            }
            EVENT_END_FILE => handle_end_file(client, event, state, sink),
            EVENT_QUEUE_OVERFLOW => sink(PlaybackEvent::Warning(
                "MPV event queue overflowed; playback state may be stale".to_owned(),
            )),
            EVENT_SHUTDOWN => {
                *running = false;
                return;
            }
            _ => {}
        }
    }
}

fn handle_end_file(
    client: &MpvClient,
    event: &MpvEvent,
    state: &mut PlaybackState,
    sink: &Arc<dyn Fn(PlaybackEvent) + Send + Sync>,
) {
    if event.data.is_null() {
        return;
    }
    // SAFETY: EVENT_END_FILE data is mpv_event_end_file for this event lifetime.
    let data = unsafe { &*(event.data as *const MpvEventEndFile) };
    let reason = match data.reason {
        END_FILE_EOF => EndReason::Eof,
        END_FILE_STOP => EndReason::Stopped,
        END_FILE_QUIT => EndReason::Quit,
        END_FILE_ERROR => EndReason::Error,
        END_FILE_REDIRECT => EndReason::Redirect,
        _ => EndReason::Unknown,
    };
    let error_code = (data.reason == END_FILE_ERROR).then_some(data.error);
    let error = error_code.map(|code| client.api.operation_error(code).to_string());
    state.loading = false;
    state.loaded = false;
    sink(PlaybackEvent::State(Box::new(state.clone())));
    sink(PlaybackEvent::Ended {
        reason,
        error_code,
        error,
    });
}

fn update_property(event: &MpvEvent, state: &mut PlaybackState) {
    if event.data.is_null() {
        return;
    }
    // SAFETY: PROPERTY_CHANGE data has this layout for the event lifetime.
    let property = unsafe { &*(event.data as *const MpvEventProperty) };
    if property.name.is_null() || property.format == FORMAT_NONE {
        return;
    }
    // SAFETY: MPV property names are null-terminated strings.
    let name = unsafe { CStr::from_ptr(property.name) }.to_string_lossy();
    match name.as_ref() {
        "pause" => state.paused = property_flag(property).unwrap_or(state.paused),
        "time-pos" => state.time = property_double(property).unwrap_or(state.time).max(0.0),
        "duration" => state.duration = property_double(property).unwrap_or(state.duration).max(0.0),
        "demuxer-cache-time" => {
            state.buffered_until = property_double(property)
                .unwrap_or(state.buffered_until)
                .max(0.0)
        }
        "paused-for-cache" => state.buffering = property_flag(property).unwrap_or(state.buffering),
        "seeking" => state.seeking = property_flag(property).unwrap_or(state.seeking),
        "volume" => {
            state.volume =
                (property_double(property).unwrap_or(state.volume * 100.0) / 100.0).clamp(0.0, 1.0)
        }
        "mute" => state.muted = property_flag(property).unwrap_or(state.muted),
        "speed" => state.speed = property_double(property).unwrap_or(state.speed),
        "aid" => state.active_audio_track = property_string(property),
        "sid" => state.active_subtitle_track = property_string(property),
        "filename" => state.filename = property_string(property),
        "file-size" => {
            state.file_size = property_int64(property).and_then(|size| u64::try_from(size).ok())
        }
        "file-format" => state.file_format = property_string(property),
        "video-format" => state.video_format = property_string(property),
        "audio-codec-name" => state.audio_format = property_string(property),
        "hwdec-current" => {
            let value = property_string(property);
            state.hardware_decoder = value.clone();
            state.current_hwdec = value;
        }
        "vo-configured" => {
            state.vo_configured = property_flag(property).unwrap_or(state.vo_configured)
        }
        "current-vo" => state.current_vo = property_string(property),
        "gpu-context" => state.current_gpu_context = property_string(property),
        "cache-buffering-state" => {
            state.cache_buffering_percent = property_int64(property)
                .map(|percent| percent as f64)
                .unwrap_or(state.cache_buffering_percent)
                .clamp(0.0, 100.0)
        }
        "track-list" => {
            if let Some(node) = property_node(property) {
                let (audio, subtitles, has_video) = parse_tracks(node);
                state.audio_tracks = audio;
                state.subtitle_tracks = subtitles;
                state.has_video_track = has_video;
            }
        }
        _ => {}
    }
}

fn property_flag(property: &MpvEventProperty) -> Option<bool> {
    if property.format != FORMAT_FLAG || property.data.is_null() {
        return None;
    }
    // SAFETY: FORMAT_FLAG data points to a C int for this event lifetime.
    Some(unsafe { *(property.data as *const c_int) } != 0)
}

fn property_double(property: &MpvEventProperty) -> Option<f64> {
    if property.format != FORMAT_DOUBLE || property.data.is_null() {
        return None;
    }
    // SAFETY: FORMAT_DOUBLE data points to a double for this event lifetime.
    Some(unsafe { *(property.data as *const f64) })
}

fn property_int64(property: &MpvEventProperty) -> Option<i64> {
    if property.format != FORMAT_INT64 || property.data.is_null() {
        return None;
    }
    // SAFETY: FORMAT_INT64 data points to int64_t for this event lifetime.
    Some(unsafe { *(property.data as *const i64) })
}

fn property_string(property: &MpvEventProperty) -> Option<String> {
    if property.format != FORMAT_STRING || property.data.is_null() {
        return None;
    }
    // SAFETY: FORMAT_STRING event data points to a char pointer.
    let value = unsafe { *(property.data as *const *const c_char) };
    if value.is_null() {
        None
    } else {
        // SAFETY: MPV provides a null-terminated string for the event lifetime.
        Some(
            unsafe { CStr::from_ptr(value) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

fn property_node(property: &MpvEventProperty) -> Option<&MpvNode> {
    if property.format != FORMAT_NODE || property.data.is_null() {
        None
    } else {
        // SAFETY: FORMAT_NODE data points to an mpv_node for the event lifetime.
        Some(unsafe { &*(property.data as *const MpvNode) })
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::{
        NativeWindowTarget, PlaybackRuntime, PlayerConfig, PlayerVideoOutput,
        copy_safe_hardware_decoding, video_options,
    };
    use std::num::NonZeroU32;

    #[test]
    fn hardware_decoding_should_use_copy_safe_windows_backends() {
        assert_eq!(copy_safe_hardware_decoding(), "d3d11va-copy,auto-copy");
    }

    #[test]
    fn native_output_uses_strict_vos_and_direct_d3d11_decoding() {
        let target = NonZeroU32::new(42).expect("test HWND");
        let options = video_options(
            &PlayerVideoOutput::NativeWindow(NativeWindowTarget::Win32(target)),
            true,
        );
        assert!(options.contains(&("vo", "gpu-next,gpu,direct3d".to_owned())));
        assert!(options.contains(&("hwdec", "d3d11va,auto-safe".to_owned())));
        assert!(
            !options
                .iter()
                .any(|(name, value)| name == &"vo" && value.ends_with(','))
        );
    }

    #[test]
    fn playback_runtime_should_start_with_pinned_dll() {
        let runtime = PlaybackRuntime::start(
            PlayerConfig {
                config_dir: None,
                hardware_decoding: false,
                video_output: PlayerVideoOutput::OpenGlRenderApi,
            },
            |_| {},
        )
        .expect("the pinned MPV DLL runtime should start");

        runtime
            .shutdown()
            .expect("the MPV actor should shut down cleanly");
    }
}

fn parse_tracks(node: &MpvNode) -> (Vec<AudioTrack>, Vec<SubtitleTrack>, bool) {
    let Some(entries) = node_list(node, FORMAT_NODE_ARRAY) else {
        return (Vec::new(), Vec::new(), false);
    };
    let mut audio = Vec::new();
    let mut subtitles = Vec::new();
    let mut has_video = false;
    for entry in entries {
        let Some(map) = node_map(entry) else {
            continue;
        };
        let kind = map_string(map, "type");
        let id = map_int(map, "id").map(|id| id.to_string());
        let Some(id) = id else { continue };
        match kind.as_deref() {
            Some("video") => has_video = true,
            Some("audio") => audio.push(AudioTrack {
                id,
                title: map_string(map, "title"),
                language: map_string(map, "lang"),
                codec: map_string(map, "codec"),
                selected: map_flag(map, "selected").unwrap_or(false),
            }),
            Some("sub") => subtitles.push(SubtitleTrack {
                id,
                title: map_string(map, "title"),
                language: map_string(map, "lang"),
                codec: map_string(map, "codec"),
                selected: map_flag(map, "selected").unwrap_or(false),
                external: map_flag(map, "external").unwrap_or(false),
            }),
            _ => {}
        }
    }
    (audio, subtitles, has_video)
}

fn node_list(node: &MpvNode, expected_format: c_int) -> Option<&[MpvNode]> {
    if node.format != expected_format {
        return None;
    }
    // SAFETY: The active union member for NODE_ARRAY/NODE_MAP is `list`.
    let list = unsafe { node.value.list };
    if list.is_null() {
        return Some(&[]);
    }
    // SAFETY: MPV guarantees num non-negative and values contains num entries.
    let list = unsafe { &*list };
    let len = usize::try_from(list.num).ok()?;
    if len == 0 {
        Some(&[])
    } else if list.values.is_null() {
        None
    } else {
        // SAFETY: Validated non-null values and MPV-provided length.
        Some(unsafe { std::slice::from_raw_parts(list.values, len) })
    }
}

fn node_map(node: &MpvNode) -> Option<&MpvNodeList> {
    if node.format != FORMAT_NODE_MAP {
        return None;
    }
    // SAFETY: The active union member for NODE_MAP is `list`.
    let list = unsafe { node.value.list };
    (!list.is_null()).then(|| unsafe { &*list })
}

fn map_value<'a>(map: &'a MpvNodeList, wanted: &str) -> Option<&'a MpvNode> {
    let len = usize::try_from(map.num).ok()?;
    if len == 0 || map.keys.is_null() || map.values.is_null() {
        return None;
    }
    for index in 0..len {
        // SAFETY: MPV guarantees key/value arrays contain num entries.
        let key = unsafe { *map.keys.add(index) };
        if key.is_null() {
            continue;
        }
        // SAFETY: Map keys are null-terminated strings.
        if unsafe { CStr::from_ptr(key) }.to_bytes() == wanted.as_bytes() {
            // SAFETY: Values has the same validated length as keys.
            return Some(unsafe { &*map.values.add(index) });
        }
    }
    None
}

fn map_string(map: &MpvNodeList, key: &str) -> Option<String> {
    let node = map_value(map, key)?;
    if node.format != FORMAT_STRING {
        return None;
    }
    // SAFETY: Active union member for FORMAT_STRING is string.
    let value = unsafe { node.value.string };
    (!value.is_null()).then(|| {
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned()
    })
}

fn map_int(map: &MpvNodeList, key: &str) -> Option<i64> {
    let node = map_value(map, key)?;
    (node.format == FORMAT_INT64).then_some(unsafe { node.value.int64 })
}

fn map_flag(map: &MpvNodeList, key: &str) -> Option<bool> {
    let node = map_value(map, key)?;
    (node.format == FORMAT_FLAG).then_some(unsafe { node.value.flag != 0 })
}
