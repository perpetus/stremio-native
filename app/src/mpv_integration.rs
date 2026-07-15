use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, anyhow};
use playback_mpv::{
    EndReason, PlaybackCommand, PlaybackController, PlaybackEvent, PlaybackRuntime, PlaybackState,
    PlayerConfig, RenderContext, RenderOutcome, RenderSource,
};
use slint::{
    BorrowedOpenGLTextureBuilder, BorrowedOpenGLTextureOrigin, ComponentHandle, ModelRc,
    SharedString, VecModel,
};
use stremio_core::{
    models::{
        common::Loadable,
        player::{Player, Selected, VideoParams},
    },
    runtime::{
        Runtime, RuntimeAction,
        msg::{Action, ActionLoad, ActionPlayer, ActionStreamingServer},
    },
    types::{
        addon::ResourcePath,
        resource::StreamSource,
        streaming_server::StatisticsRequest,
        streams::{AudioTrack, StreamItemState, SubtitleTrack},
    },
};
use tokio_util::sync::CancellationToken;

use crate::{AppModel, AppModelField, MainWindow};
use core_env::DesktopEnv;

const PLAYER_DEVICE: &str = "libmpv";

#[derive(Default)]
struct SessionState {
    url: Option<String>,
    loaded_subtitles: HashSet<String>,
    last_time: u64,
    last_time_dispatch: Option<Instant>,
    last_paused: Option<bool>,
    last_video_params: Option<VideoParams>,
    load_requested_at: Option<Instant>,
}

struct StatisticsPoll {
    key: (String, u16),
    cancellation: CancellationToken,
}

#[derive(Clone)]
pub struct NativePlaybackBridge {
    controller: PlaybackController,
    core: Arc<Runtime<DesktopEnv, AppModel>>,
    state: Arc<RwLock<PlaybackState>>,
    session: Arc<Mutex<SessionState>>,
    statistics_poll: Arc<Mutex<Option<StatisticsPoll>>>,
}

pub struct NativePlayback {
    runtime: PlaybackRuntime,
    bridge: NativePlaybackBridge,
}

impl NativePlayback {
    pub fn start(
        ui: &MainWindow,
        core: &Arc<Runtime<DesktopEnv, AppModel>>,
        hardware_decoding: bool,
    ) -> anyhow::Result<Self> {
        let state = Arc::new(RwLock::new(PlaybackState::default()));
        let session = Arc::new(Mutex::new(SessionState::default()));
        let statistics_poll = Arc::new(Mutex::new(None));
        let controller_slot = Arc::new(OnceLock::<PlaybackController>::new());
        let ui_update_pending = Arc::new(AtomicBool::new(false));

        let event_state = state.clone();
        let event_session = session.clone();
        let event_core = core.clone();
        let event_ui = ui.as_weak();
        let event_controller = controller_slot.clone();
        let event_pending = ui_update_pending.clone();
        let config_dir = resolve_config_dir();
        tracing::info!(
            hardware_decoding,
            config_dir = %config_dir.display(),
            "initializing native MPV playback"
        );
        std::fs::create_dir_all(&config_dir).with_context(|| {
            format!(
                "could not create MPV config directory {}",
                config_dir.display()
            )
        })?;
        let runtime = PlaybackRuntime::start(
            PlayerConfig {
                config_dir: Some(config_dir),
                hardware_decoding,
            },
            move |event| {
                handle_event(
                    event,
                    &event_state,
                    &event_session,
                    &event_core,
                    &event_controller,
                    &event_ui,
                    &event_pending,
                );
            },
        )
        .context("could not initialize the MPV playback engine")?;
        let controller = runtime.controller();
        controller_slot
            .set(controller.clone())
            .map_err(|_| anyhow!("MPV controller was initialized twice"))?;
        if let Ok(model) = core.model() {
            let settings = &model.ctx.profile.settings;
            log_command(controller.send(PlaybackCommand::SetSubtitleScale(
                f64::from(settings.subtitles_size) / 100.0,
            )));
            log_command(
                controller.send(PlaybackCommand::SetSubtitlePosition(f64::from(
                    100_u8.saturating_sub(settings.subtitles_offset),
                ))),
            );
            ui.set_player_seek_step_seconds(settings.seek_time_duration as f32 / 1_000.0);
            ui.set_player_subtitle_size_percent(f32::from(settings.subtitles_size));
            ui.set_player_subtitle_offset_percent(f32::from(settings.subtitles_offset));
        }
        install_renderer(
            ui,
            runtime.render_source(),
            state.clone(),
            session.clone(),
        )?;
        tracing::info!("native MPV playback initialized");

        let bridge = NativePlaybackBridge {
            controller,
            core: core.clone(),
            state,
            session,
            statistics_poll,
        };
        bridge.install_callbacks(ui, core);
        Ok(Self { runtime, bridge })
    }

    pub fn bridge(&self) -> NativePlaybackBridge {
        self.bridge.clone()
    }

    pub fn shutdown(self) -> anyhow::Result<()> {
        self.bridge.cancel_statistics_poll();
        self.runtime.shutdown().map_err(Into::into)
    }
}

impl NativePlaybackBridge {
    #[tracing::instrument(skip_all)]
    pub fn sync_player(&self, player: &Player, ui: &slint::Weak<MainWindow>) {
        let _span = tracing::info_span!("sync_player").entered();
        self.sync_statistics_poll(player);
        let Some(Loadable::Ready((stream_urls, _))) = player.stream.as_ref() else {
            if let Some(Loadable::Err(error)) = player.stream.as_ref() {
                show_player_error(ui, format!("Could not resolve this stream: {error}"));
            }
            return;
        };
        let Some(url) = stream_urls.streaming_url.as_ref().map(ToString::to_string) else {
            show_player_error(
                ui,
                "This stream does not provide a playable URL.".to_owned(),
            );
            return;
        };

        let start_at = resume_time(player);
        let should_load = {
            let mut session = lock_session(&self.session);
            if session.url.as_deref() == Some(url.as_str()) {
                false
            } else {
                session.url = Some(url.clone());
                session.loaded_subtitles.clear();
                session.last_time = start_at.unwrap_or_default().round().max(0.0) as u64;
                session.last_time_dispatch = None;
                session.last_paused = None;
                session.last_video_params = None;
                session.load_requested_at = Some(Instant::now());
                true
            }
        };
        if should_load {
            let ui_for_update = ui.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_for_update.upgrade() {
                    ui.set_player_error("".into());
                    ui.set_player_video_frame(slint::Image::default());
                    ui.set_player_has_video_frame(false);
                    ui.set_player_loading(true);
                    ui.set_player_buffering(false);
                    ui.set_player_buffering_percent(0.0);
                    ui.set_show_player(true);
                }
            });
            send_or_show(
                &self.controller,
                PlaybackCommand::Load { url, start_at },
                ui,
            );
        }

        for resource in &player.subtitles {
            let Some(Loadable::Ready(subtitles)) = resource.content.as_ref() else {
                continue;
            };
            for subtitle in subtitles {
                let should_add = lock_session(&self.session)
                    .loaded_subtitles
                    .insert(subtitle.id.clone());
                if should_add {
                    send_or_show(
                        &self.controller,
                        PlaybackCommand::AddSubtitle {
                            url: subtitle.url.to_string(),
                            title: subtitle
                                .label
                                .clone()
                                .or_else(|| Some(subtitle.lang.clone())),
                        },
                        ui,
                    );
                }
            }
        }
    }

    fn install_callbacks(&self, ui: &MainWindow, core: &Arc<Runtime<DesktopEnv, AppModel>>) {
        ui.on_player_toggle_pause({
            let controller = self.controller.clone();
            move || log_command(controller.send(PlaybackCommand::TogglePaused))
        });

        ui.on_player_seek({
            let controller = self.controller.clone();
            let state = self.state.clone();
            let session = self.session.clone();
            let core = core.clone();
            move |progress| {
                let state = read_state(&state).clone();
                let time = state.duration * f64::from(progress.clamp(0.0, 1.0));
                log_command(controller.send(PlaybackCommand::SeekAbsolute(time)));
                lock_session(&session).last_time = time.round().max(0.0) as u64;
                dispatch_player(
                    &core,
                    ActionPlayer::Seek {
                        time: time.round().max(0.0) as u64,
                        duration: state.duration.round().max(0.0) as u64,
                        device: PLAYER_DEVICE.to_owned(),
                    },
                );
            }
        });

        ui.on_player_seek_relative({
            let controller = self.controller.clone();
            let state = self.state.clone();
            let session = self.session.clone();
            let core = core.clone();
            move |seconds| {
                let state = read_state(&state).clone();
                let time = (state.time + f64::from(seconds)).clamp(0.0, state.duration.max(0.0));
                log_command(controller.send(PlaybackCommand::SeekRelative(f64::from(seconds))));
                lock_session(&session).last_time = time.round().max(0.0) as u64;
                dispatch_player(
                    &core,
                    ActionPlayer::Seek {
                        time: time.round().max(0.0) as u64,
                        duration: state.duration.round().max(0.0) as u64,
                        device: PLAYER_DEVICE.to_owned(),
                    },
                );
            }
        });

        ui.on_player_change_volume({
            let controller = self.controller.clone();
            move |volume| {
                log_command(controller.send(PlaybackCommand::SetVolume(f64::from(volume))))
            }
        });

        ui.on_player_toggle_mute({
            let controller = self.controller.clone();
            let state = self.state.clone();
            move || {
                let muted = !read_state(&state).muted;
                log_command(controller.send(PlaybackCommand::SetMuted(muted)));
            }
        });

        ui.on_player_change_audio({
            let controller = self.controller.clone();
            let state = self.state.clone();
            let core = core.clone();
            move |index| {
                let track = usize::try_from(index)
                    .ok()
                    .and_then(|index| read_state(&state).audio_tracks.get(index).cloned());
                let track_id = track.as_ref().map(|track| track.id.clone());
                log_command(controller.send(PlaybackCommand::SetAudioTrack(track_id)));
                update_stream_state(&core, |stream_state| {
                    stream_state.audio_track = track.map(|track| AudioTrack {
                        id: track.id,
                        language: track.language,
                    });
                });
            }
        });

        ui.on_player_change_subtitle({
            let controller = self.controller.clone();
            let state = self.state.clone();
            let core = core.clone();
            move |index| {
                let track = usize::try_from(index)
                    .ok()
                    .and_then(|index| read_state(&state).subtitle_tracks.get(index).cloned());
                let track_id = track.as_ref().map(|track| track.id.clone());
                log_command(controller.send(PlaybackCommand::SetSubtitleTrack(track_id)));
                update_stream_state(&core, |stream_state| {
                    stream_state.subtitle_track = track.map(|track| SubtitleTrack {
                        id: track.id,
                        embedded: !track.external,
                        language: track.language,
                    });
                });
            }
        });

        ui.on_player_change_subtitle_delay({
            let controller = self.controller.clone();
            let core = core.clone();
            move |seconds| {
                let milliseconds = (f64::from(seconds) * 1_000.0).round() as i64;
                log_command(controller.send(PlaybackCommand::SetSubtitleDelay(milliseconds)));
                update_stream_state(&core, |stream_state| {
                    stream_state.subtitle_delay = Some(milliseconds);
                });
            }
        });

        ui.on_player_change_subtitle_size({
            let controller = self.controller.clone();
            let core = core.clone();
            move |percent| {
                let percent = percent.clamp(50.0, 200.0);
                log_command(controller.send(PlaybackCommand::SetSubtitleScale(
                    f64::from(percent) / 100.0,
                )));
                update_stream_state(&core, |stream_state| {
                    stream_state.subtitle_size = Some(percent);
                });
            }
        });

        ui.on_player_change_subtitle_offset({
            let controller = self.controller.clone();
            let core = core.clone();
            move |percent| {
                let percent = percent.clamp(0.0, 100.0);
                log_command(
                    controller.send(PlaybackCommand::SetSubtitlePosition(f64::from(
                        100.0 - percent,
                    ))),
                );
                update_stream_state(&core, |stream_state| {
                    stream_state.subtitle_offset = Some(percent);
                });
            }
        });

        ui.on_player_change_speed({
            let controller = self.controller.clone();
            let core = core.clone();
            move |speed| {
                log_command(controller.send(PlaybackCommand::SetSpeed(f64::from(speed))));
                update_stream_state(&core, |stream_state| {
                    stream_state.playback_speed = Some(speed);
                });
            }
        });

        ui.on_player_change_video_scale({
            let controller = self.controller.clone();
            move |mode| {
                let mode = u8::try_from(mode).unwrap_or_default() % 3;
                log_command(controller.send(PlaybackCommand::SetVideoScale(mode)));
            }
        });

        ui.on_player_copy_stream_link({
            let session = self.session.clone();
            move || {
                let url = lock_session(&session).url.clone();
                let Some(url) = url else {
                    tracing::warn!("cannot copy stream link before a stream is loaded");
                    return;
                };
                match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(url)) {
                    Ok(()) => tracing::info!("stream link copied to clipboard"),
                    Err(error) => tracing::error!(%error, "failed to copy stream link"),
                }
            }
        });

        ui.on_player_download_video({
            let session = self.session.clone();
            move || {
                let url = lock_session(&session).url.clone();
                let Some(url) = url else {
                    tracing::warn!("cannot download video before a stream is loaded");
                    return;
                };
                if let Err(error) = open::that(&url) {
                    tracing::error!(%error, %url, "failed to open video download URL");
                }
            }
        });

        ui.on_player_play_episode({
            let core = core.clone();
            let controller = self.controller.clone();
            let weak = ui.as_weak();
            let statistics_poll = self.statistics_poll.clone();
            let session = self.session.clone();
            move |index| {
                let current = weak
                    .upgrade()
                    .map(|ui| ui.get_player_active_episode_idx())
                    .unwrap_or(-1);
                if index == current + 1 {
                    play_next(&core);
                    return;
                }

                unload_player(&controller, &core, &statistics_poll, &session);
                if let Some(ui) = weak.upgrade() {
                    ui.set_player_active_episode_idx(index);
                    ui.set_detail_active_episode_idx(index);
                    ui.invoke_details_episode_changed(index);
                    ui.set_show_player(false);
                    ui.set_player_loading(false);
                    ui.set_player_buffering(false);
                    ui.set_player_has_video_frame(false);
                    ui.set_player_video_frame(slint::Image::default());
                    if ui.window().is_fullscreen() {
                        ui.window().set_fullscreen(false);
                    }
                }
            }
        });

        ui.on_player_close({
            let controller = self.controller.clone();
            let core = core.clone();
            let weak = ui.as_weak();
            let statistics_poll = self.statistics_poll.clone();
            let session = self.session.clone();
            move || {
                unload_player(&controller, &core, &statistics_poll, &session);
                if let Some(ui) = weak.upgrade() {
                    ui.set_show_player(false);
                    ui.set_player_loading(false);
                    ui.set_player_buffering(false);
                    ui.set_player_has_video_frame(false);
                    ui.set_player_video_frame(slint::Image::default());
                    if ui.window().is_fullscreen() {
                        ui.window().set_fullscreen(false);
                        ui.set_is_fullscreen(false);
                    }
                }
            }
        });

        ui.on_player_toggle_fullscreen({
            let weak = ui.as_weak();
            move || {
                if let Some(ui) = weak.upgrade() {
                    let fs = !ui.window().is_fullscreen();
                    ui.window().set_fullscreen(fs);
                    ui.set_is_fullscreen(fs);
                }
            }
        });
    }

    fn sync_statistics_poll(&self, player: &Player) {
        let request = player.selected.as_ref().and_then(|selected| {
            let StreamSource::Torrent {
                info_hash,
                file_idx,
                ..
            } = &selected.stream.source
            else {
                return None;
            };
            Some(StatisticsRequest {
                info_hash: info_hash.iter().map(|byte| format!("{byte:02x}")).collect(),
                file_idx: file_idx.unwrap_or_default(),
            })
        });
        let Some(request) = request else {
            self.cancel_statistics_poll();
            return;
        };
        let key = (request.info_hash.clone(), request.file_idx);
        {
            let current = lock_statistics_poll(&self.statistics_poll);
            if current.as_ref().is_some_and(|poll| poll.key == key) {
                return;
            }
        }
        self.cancel_statistics_poll();
        let cancellation = CancellationToken::new();
        *lock_statistics_poll(&self.statistics_poll) = Some(StatisticsPoll {
            key,
            cancellation: cancellation.clone(),
        });
        let core_request = request.clone();
        let core = self.core.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = cancellation.cancelled() => break,
                    _ = interval.tick() => core.dispatch(RuntimeAction {
                        field: None,
                        action: Action::StreamingServer(
                            ActionStreamingServer::GetStatistics(core_request.clone())
                        ),
                    }),
                }
            }
        });
    }

    fn cancel_statistics_poll(&self) {
        cancel_statistics_poll(&self.statistics_poll);
    }
}

fn handle_event(
    event: PlaybackEvent,
    state_slot: &Arc<RwLock<PlaybackState>>,
    session: &Arc<Mutex<SessionState>>,
    core: &Arc<Runtime<DesktopEnv, AppModel>>,
    controller: &Arc<OnceLock<PlaybackController>>,
    ui: &slint::Weak<MainWindow>,
    ui_update_pending: &Arc<AtomicBool>,
) {
    match event {
        PlaybackEvent::State(state) => {
            let state = *state;
            let previous = read_state(state_slot).clone();
            if previous.loading != state.loading
                || previous.loaded != state.loaded
                || previous.paused != state.paused
                || previous.buffering != state.buffering
                || previous.seeking != state.seeking
            {
                tracing::info!(
                    loading = state.loading,
                    loaded = state.loaded,
                    paused = state.paused,
                    buffering = state.buffering,
                    seeking = state.seeking,
                    duration_seconds = state.duration,
                    "MPV playback state changed"
                );
            }
            match state_slot.write() {
                Ok(mut current) => *current = state.clone(),
                Err(poisoned) => *poisoned.into_inner() = state.clone(),
            }
            dispatch_state_to_core(&state, session, core);
            schedule_ui_state(ui, state_slot, ui_update_pending);
        }
        PlaybackEvent::FileLoaded => {
            let load_elapsed_ms = lock_session(session)
                .load_requested_at
                .map(|started_at| started_at.elapsed().as_millis());
            tracing::info!(?load_elapsed_ms, "MPV file loaded");
            restore_stream_state(core, controller, ui);
        }
        PlaybackEvent::Ended { reason, error } => {
            tracing::info!(?reason, error = error.as_deref(), "MPV playback ended");
            if reason == EndReason::Eof {
                dispatch_player(core, ActionPlayer::Ended);
                let binge = core
                    .model()
                    .ok()
                    .map(|model| model.ctx.profile.settings.binge_watching)
                    .unwrap_or(false);
                if binge && play_next(core) {
                    let ui = ui.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui.upgrade() {
                            ui.set_player_active_episode_idx(
                                ui.get_player_active_episode_idx() + 1,
                            );
                        }
                    });
                }
            } else if let Some(error) = error {
                show_player_error(ui, error);
            }
        }
        PlaybackEvent::Warning(error) => tracing::warn!(%error, "MPV command failed"),
        PlaybackEvent::Error(error) => {
            tracing::error!(%error, "MPV playback error");
            show_player_error(ui, error);
        }
        PlaybackEvent::Shutdown => tracing::info!("MPV playback shutdown event received"),
    }
}

fn dispatch_state_to_core(
    state: &PlaybackState,
    session: &Arc<Mutex<SessionState>>,
    core: &Arc<Runtime<DesktopEnv, AppModel>>,
) {
    let mut session = lock_session(session);
    if session.last_paused != Some(state.paused) {
        session.last_paused = Some(state.paused);
        dispatch_player(
            core,
            ActionPlayer::PausedChanged {
                paused: state.paused,
            },
        );
    }

    let now = Instant::now();
    let time = state.time.round().max(0.0) as u64;
    let should_dispatch_time = state.loaded
        && !state.seeking
        && time >= session.last_time
        && session
            .last_time_dispatch
            .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(1));
    if should_dispatch_time {
        session.last_time = time;
        session.last_time_dispatch = Some(now);
        dispatch_player(
            core,
            ActionPlayer::TimeChanged {
                time,
                duration: state.duration.round().max(0.0) as u64,
                device: PLAYER_DEVICE.to_owned(),
            },
        );
    }

    let hash = core.model().ok().and_then(|model| {
        model
            .player
            .stream
            .as_ref()
            .and_then(Loadable::ready)
            .and_then(|(_, stream)| stream.behavior_hints.video_hash.clone())
    });
    let params = VideoParams {
        hash,
        size: state.file_size,
        filename: state.filename.clone(),
    };
    if session.last_video_params.as_ref() != Some(&params)
        && (params.hash.is_some() || params.size.is_some() || params.filename.is_some())
    {
        session.last_video_params = Some(params.clone());
        dispatch_player(
            core,
            ActionPlayer::VideoParamsChanged {
                video_params: Some(params),
            },
        );
    }
}

fn restore_stream_state(
    core: &Arc<Runtime<DesktopEnv, AppModel>>,
    controller: &Arc<OnceLock<PlaybackController>>,
    ui: &slint::Weak<MainWindow>,
) {
    let Some(controller) = controller.get() else {
        return;
    };
    let stream_state = core
        .model()
        .ok()
        .and_then(|model| model.player.stream_state.clone());
    let Some(stream_state) = stream_state else {
        return;
    };
    if let Some(speed) = stream_state.playback_speed {
        log_command(controller.send(PlaybackCommand::SetSpeed(f64::from(speed))));
    }
    if let Some(audio) = stream_state.audio_track {
        log_command(controller.send(PlaybackCommand::SetAudioTrack(Some(audio.id))));
    }
    if let Some(subtitle) = stream_state.subtitle_track {
        log_command(controller.send(PlaybackCommand::SetSubtitleTrack(Some(subtitle.id))));
    }
    if let Some(delay) = stream_state.subtitle_delay {
        log_command(controller.send(PlaybackCommand::SetSubtitleDelay(delay)));
    }
    if let Some(scale) = stream_state.subtitle_size {
        log_command(controller.send(PlaybackCommand::SetSubtitleScale(f64::from(scale) / 100.0)));
    }
    if let Some(offset) = stream_state.subtitle_offset {
        log_command(
            controller.send(PlaybackCommand::SetSubtitlePosition(f64::from(
                100.0 - offset.clamp(0.0, 100.0),
            ))),
        );
    }
    if let Some(delay) = stream_state.audio_delay {
        log_command(controller.send(PlaybackCommand::SetAudioDelay(delay)));
    }

    let weak = ui.clone();
    let subtitle_delay = stream_state.subtitle_delay.unwrap_or_default() as f32 / 1_000.0;
    let subtitle_size = stream_state.subtitle_size.unwrap_or(100.0);
    let subtitle_offset = stream_state.subtitle_offset.unwrap_or(100.0);
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_player_subtitle_delay_seconds(subtitle_delay);
            ui.set_player_subtitle_size_percent(subtitle_size);
            ui.set_player_subtitle_offset_percent(subtitle_offset);
        }
    });
}

fn schedule_ui_state(
    ui: &slint::Weak<MainWindow>,
    state: &Arc<RwLock<PlaybackState>>,
    pending: &Arc<AtomicBool>,
) {
    if pending.swap(true, Ordering::AcqRel) {
        return;
    }
    let ui = ui.clone();
    let state = state.clone();
    let pending = pending.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let snapshot = read_state(&state).clone();
        if let Some(ui) = ui.upgrade() {
            apply_state_to_ui(&ui, &snapshot);
        }
        pending.store(false, Ordering::Release);
    });
}

fn apply_state_to_ui(ui: &MainWindow, state: &PlaybackState) {
    ui.set_player_loading(state.loading);
    ui.set_player_buffering(state.buffering);
    ui.set_player_buffering_percent(state.cache_buffering_percent as f32);
    ui.set_player_paused(state.paused);
    ui.set_player_volume(state.volume as f32);
    ui.set_player_muted(state.muted);
    ui.set_player_playback_speed(state.speed as f32);
    ui.set_player_progress(if state.duration > 0.0 {
        (state.time / state.duration).clamp(0.0, 1.0) as f32
    } else {
        0.0
    });
    ui.set_player_elapsed_time_str(format_time(state.time).into());
    ui.set_player_total_time_str(format_time(state.duration).into());

    let audio_labels = state
        .audio_tracks
        .iter()
        .map(|track| track_label(&track.title, &track.language, &track.codec))
        .map(SharedString::from)
        .collect::<Vec<_>>();
    ui.set_player_audio_tracks(ModelRc::new(VecModel::from(audio_labels)));
    let audio_language_labels = state
        .audio_tracks
        .iter()
        .map(|track| language_label(track.language.as_deref()))
        .map(SharedString::from)
        .collect::<Vec<_>>();
    ui.set_player_audio_track_languages(ModelRc::new(VecModel::from(audio_language_labels)));
    let audio_detail_labels = state
        .audio_tracks
        .iter()
        .map(|track| {
            track
                .title
                .as_ref()
                .or(track.codec.as_ref())
                .cloned()
                .unwrap_or_else(|| "Audio track".to_owned())
        })
        .map(SharedString::from)
        .collect::<Vec<_>>();
    ui.set_player_audio_track_labels(ModelRc::new(VecModel::from(audio_detail_labels)));
    ui.set_player_active_audio_idx(
        state
            .active_audio_track
            .as_ref()
            .and_then(|active| {
                state
                    .audio_tracks
                    .iter()
                    .position(|track| &track.id == active)
            })
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1),
    );

    let subtitle_labels = state
        .subtitle_tracks
        .iter()
        .map(|track| track_label(&track.title, &track.language, &track.codec))
        .map(SharedString::from)
        .collect::<Vec<_>>();
    ui.set_player_subtitles_tracks(ModelRc::new(VecModel::from(subtitle_labels)));
    let subtitle_track_languages = state
        .subtitle_tracks
        .iter()
        .map(|track| language_label(track.language.as_deref()))
        .collect::<Vec<_>>();
    ui.set_player_subtitle_track_languages(ModelRc::new(VecModel::from(
        subtitle_track_languages
            .iter()
            .cloned()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    let subtitle_track_origins = state
        .subtitle_tracks
        .iter()
        .map(|track| {
            if track.external {
                "External"
            } else {
                "Embedded"
            }
        })
        .map(SharedString::from)
        .collect::<Vec<_>>();
    ui.set_player_subtitle_track_origins(ModelRc::new(VecModel::from(subtitle_track_origins)));
    let mut subtitle_languages = Vec::<SharedString>::new();
    let mut subtitle_language_track_indices = Vec::<i32>::new();
    for (index, language) in subtitle_track_languages.iter().enumerate() {
        if subtitle_languages
            .iter()
            .any(|existing| existing.as_str() == language)
        {
            continue;
        }
        subtitle_languages.push(language.clone().into());
        if let Ok(index) = i32::try_from(index) {
            subtitle_language_track_indices.push(index);
        }
    }
    ui.set_player_subtitle_languages(ModelRc::new(VecModel::from(subtitle_languages)));
    ui.set_player_subtitle_language_track_indices(ModelRc::new(VecModel::from(
        subtitle_language_track_indices,
    )));
    ui.set_player_active_subtitle_idx(
        state
            .active_subtitle_track
            .as_ref()
            .and_then(|active| {
                state
                    .subtitle_tracks
                    .iter()
                    .position(|track| &track.id == active)
            })
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1),
    );
    ui.set_player_video_format(state.video_format.clone().unwrap_or_default().into());
    ui.set_player_audio_format(state.audio_format.clone().unwrap_or_default().into());
    ui.set_player_file_format(state.file_format.clone().unwrap_or_default().into());
    ui.set_player_hwdec(state.hardware_decoder.clone().unwrap_or_default().into());
    ui.set_player_buffered_percent(if state.duration > 0.0 {
        ((state.buffered_until / state.duration) * 100.0).clamp(0.0, 100.0) as f32
    } else {
        0.0
    });
}

fn install_renderer(
    ui: &MainWindow,
    source: RenderSource,
    playback_state: Arc<RwLock<PlaybackState>>,
    session: Arc<Mutex<SessionState>>,
) -> anyhow::Result<()> {
    let backend = std::env::var("SLINT_BACKEND").unwrap_or_else(|_| "NOT SET".to_owned());
    tracing::info!(backend = %backend, "install_renderer called");
    let window_weak = ui.as_weak();
    let mut context: Option<RenderContext> = None;
    let mut render_target_ready = false;
    let mut initial_surface_logged = false;
    let mut last_reported_load = None;
    let mut last_gl_error_logged = None;
    let mut last_player_visible = None;
    let mut allocated_size: Option<(i32, i32)> = None;
    let mut pending_size: Option<(i32, i32)> = None;
    let mut pending_size_since = Instant::now();
    ui.window()
        .set_rendering_notifier(move |state, graphics_api| match state {
            slint::RenderingState::RenderingSetup => {
                tracing::info!(?graphics_api, "Slint rendering setup started");
                let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = graphics_api else {
                    tracing::error!(?graphics_api, "MPV requires Slint's NativeOpenGL renderer");
                    return;
                };
                let redraw_weak = window_weak.clone();
                match source.create_context(get_proc_address, move || {
                    crate::performance::counters().record_mpv_redraw_post();
                    let redraw_weak = redraw_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = redraw_weak.upgrade() {
                            ui.window().request_redraw();
                        }
                    });
                }) {
                    Ok(mut render_context) => {
                        let diagnostics = render_context.open_gl_diagnostics();
                        tracing::info!(
                            vendor = %diagnostics.vendor,
                            renderer = %diagnostics.renderer,
                            version = %diagnostics.version,
                            "MPV is sharing Slint's OpenGL context"
                        );
                        if let Some(ui) = window_weak.upgrade() {
                            match ensure_render_target(&ui, &mut render_context) {
                                Ok(ready) => {
                                    render_target_ready = ready;
                                    let size = ui.window().size();
                                    allocated_size = Some((
                                        i32::try_from(size.width).unwrap_or(i32::MAX),
                                        i32::try_from(size.height).unwrap_or(i32::MAX),
                                    ));
                                    pending_size = allocated_size;
                                }
                                Err(error) => {
                                    tracing::error!(%error, "MPV video render target creation failed")
                                }
                            }
                        }
                        context = Some(render_context);
                        tracing::info!("MPV OpenGL render context created");
                    }
                    Err(error) => tracing::error!(%error, "could not create MPV render context"),
                }
            }
            slint::RenderingState::BeforeRendering => {
                let Some(ui) = window_weak.upgrade() else {
                    return;
                };
                let player_visible = ui.get_show_player();
                if last_player_visible != Some(player_visible) {
                    last_player_visible = Some(player_visible);
                    tracing::info!(player_visible, "BeforeRendering: player visibility changed");
                }
                if context.is_none() && player_visible {
                    tracing::warn!("BeforeRendering: player is visible but MPV render context is None!");
                }
                if !player_visible {
                    if let Some(context) = context.as_mut()
                        && let Err(error) = context.process_updates(false)
                    {
                        tracing::error!(%error, "MPV hidden-frame update processing failed");
                    }
                    return;
                }

                if let Some(context) = context.as_mut() {
                    let size = ui.window().size();
                    let requested_size = (
                        i32::try_from(size.width).unwrap_or(i32::MAX),
                        i32::try_from(size.height).unwrap_or(i32::MAX),
                    );
                    if pending_size != Some(requested_size) {
                        pending_size = Some(requested_size);
                        pending_size_since = Instant::now();
                    }
                    let resize_settled = pending_size_since.elapsed() >= Duration::from_millis(100);
                    if !context.has_video_textures()
                        || (allocated_size != Some(requested_size) && resize_settled)
                    {
                        match ensure_render_target_size(&ui, context, requested_size) {
                            Ok(ready) => {
                                render_target_ready = ready;
                                allocated_size = Some(requested_size);
                            }
                            Err(error) => tracing::error!(
                                %error,
                                "MPV video render target creation failed"
                            ),
                        }
                    }
                } else {
                    render_target_ready = false;
                }
                if !render_target_ready {
                    return;
                }
                let size = ui.window().size();
                if let Some(context) = context.as_mut() {
                    let render_result = context.render();
                    if let Some(code) = context.take_gl_error()
                        && last_gl_error_logged != Some(code)
                    {
                        last_gl_error_logged = Some(code);
                        tracing::error!(
                            code = format_args!("{code:#x}"),
                            "OpenGL error after MPV render"
                        );
                    }
                    match render_result {
                        Ok(RenderOutcome::Rendered {
                            texture,
                            frame_ready,
                        }) => {
                            let image = unsafe {
                                BorrowedOpenGLTextureBuilder::new_gl_2d_rgba_texture(
                                    texture.texture_id(),
                                    (texture.width(), texture.height()).into(),
                                )
                            }
                            .origin(BorrowedOpenGLTextureOrigin::TopLeft)
                            .build();
                            ui.set_player_video_frame(image);
                            crate::performance::counters().record_mpv_frame_published();

                            let playable_frame = frame_ready && read_state(&playback_state).loaded;
                            if playable_frame {
                                ui.set_player_has_video_frame(true);
                            }

                            if !initial_surface_logged {
                                initial_surface_logged = true;
                                tracing::info!(
                                    width = size.width,
                                    height = size.height,
                                    texture_id = texture.texture_id().get(),
                                    "initial MPV video surface submitted to Slint"
                                );
                            }
                            let load_started_at = playable_frame
                                .then(|| lock_session(&session).load_requested_at)
                                .flatten();
                            if load_started_at.is_some() && load_started_at != last_reported_load {
                                last_reported_load = load_started_at;
                                tracing::info!(
                                    width = size.width,
                                    height = size.height,
                                    texture_id = texture.texture_id().get(),
                                    load_to_first_frame_ms = load_started_at
                                        .map(|started_at| started_at.elapsed().as_millis()),
                                    "first post-load MPV video frame submitted to Slint"
                                );
                            }
                        }
                        Ok(RenderOutcome::NoFrame) => {
                            tracing::trace!("BeforeRendering: MPV has no new frame to render");
                        }
                        Err(error) => {
                            tracing::error!(
                                %error,
                                width = size.width,
                                height = size.height,
                                "MPV frame rendering failed"
                            );
                        }
                    }
                }
            }
            slint::RenderingState::AfterRendering => {}
            slint::RenderingState::RenderingTeardown => {
                tracing::info!("Slint rendering teardown started");
                if let Some(ui) = window_weak.upgrade() {
                    ui.set_player_video_frame(slint::Image::default());
                    ui.set_player_has_video_frame(false);
                }
                context = None;
                render_target_ready = false;
                initial_surface_logged = false;
                last_reported_load = None;
                last_gl_error_logged = None;
                last_player_visible = None;
                allocated_size = None;
                pending_size = None;
            }
            _ => {}
        })
        .map_err(|error| anyhow!("Slint renderer cannot host MPV: {error}"))?;
    Ok(())
}

fn ensure_render_target(ui: &MainWindow, context: &mut RenderContext) -> anyhow::Result<bool> {
    let size = ui.window().size();
    let width = i32::try_from(size.width)?;
    let height = i32::try_from(size.height)?;
    ensure_render_target_size(ui, context, (width, height))
}

fn ensure_render_target_size(
    ui: &MainWindow,
    context: &mut RenderContext,
    (width, height): (i32, i32),
) -> anyhow::Result<bool> {
    let start = std::time::Instant::now();
    if context.ensure_video_textures(width, height)? {
        // A previous borrowed image must not outlive targets discarded by a
        // resize. The next completed frame installs the new texture.
        ui.set_player_video_frame(slint::Image::default());
        tracing::info!(
            width,
            height,
            elapsed_ms = start.elapsed().as_millis(),
            "double-buffered MPV video targets created"
        );
    }
    Ok(context.has_video_textures())
}

fn resume_time(player: &Player) -> Option<f64> {
    let selected_video = player
        .selected
        .as_ref()?
        .stream_request
        .as_ref()?
        .path
        .id
        .as_str();
    let item = player.library_item.as_ref()?;
    (item.state.video_id.as_deref() == Some(selected_video))
        .then_some(item.state.time_offset as f64)
}

fn play_next(core: &Arc<Runtime<DesktopEnv, AppModel>>) -> bool {
    let selected = core.model().ok().and_then(|model| {
        let current = model.player.selected.as_ref()?;
        let next_video = model.player.next_video.as_ref()?;
        let next_stream = model.player.next_stream.clone()?;
        let mut stream_request = current.stream_request.clone();
        if let Some(request) = stream_request.as_mut() {
            request.path.id = next_video.id.clone();
        }
        let subtitles_path = current.subtitles_path.as_ref().map(|path| ResourcePath {
            id: next_video.id.clone(),
            ..path.clone()
        });
        Some(Selected {
            stream: next_stream,
            stream_request,
            meta_request: current.meta_request.clone(),
            subtitles_path,
        })
    });
    let Some(selected) = selected else {
        return false;
    };
    dispatch_player(core, ActionPlayer::NextVideo);
    core.dispatch(RuntimeAction {
        field: None,
        action: Action::Load(ActionLoad::Player(Box::new(selected))),
    });
    true
}

fn update_stream_state(
    core: &Arc<Runtime<DesktopEnv, AppModel>>,
    update: impl FnOnce(&mut StreamItemState),
) {
    let mut state = core
        .model()
        .ok()
        .and_then(|model| model.player.stream_state.clone())
        .unwrap_or_default();
    update(&mut state);
    dispatch_player(core, ActionPlayer::StreamStateChanged { state });
}

fn dispatch_player(core: &Arc<Runtime<DesktopEnv, AppModel>>, action: ActionPlayer) {
    core.dispatch(RuntimeAction {
        field: None,
        action: Action::Player(action),
    });
}

fn resolve_config_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("StremioRust")
        .join("mpv")
}

fn format_time(seconds: f64) -> String {
    let total = seconds.round().max(0.0) as u64;
    let hours = total / 3_600;
    let minutes = (total % 3_600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn track_label(
    title: &Option<String>,
    language: &Option<String>,
    codec: &Option<String>,
) -> String {
    title
        .as_ref()
        .or(language.as_ref())
        .or(codec.as_ref())
        .cloned()
        .unwrap_or_else(|| "Unknown track".to_owned())
}

fn language_label(language: Option<&str>) -> String {
    let raw = language.map(str::trim).filter(|value| !value.is_empty());
    let Some(raw) = raw else {
        return "Unknown".to_owned();
    };
    let normalized = raw
        .split(['-', '_'])
        .next()
        .unwrap_or(raw)
        .to_ascii_lowercase();
    match normalized.as_str() {
        "ar" | "ara" => "Arabic",
        "bg" | "bul" => "Bulgarian",
        "cs" | "ces" | "cze" => "Czech",
        "da" | "dan" => "Danish",
        "de" | "deu" | "ger" => "German",
        "el" | "ell" | "gre" => "Greek",
        "en" | "eng" => "English",
        "es" | "spa" => "Spanish",
        "et" | "est" => "Estonian",
        "fa" | "fas" | "per" => "Persian",
        "fi" | "fin" => "Finnish",
        "fr" | "fra" | "fre" => "French",
        "he" | "heb" => "Hebrew",
        "hi" | "hin" => "Hindi",
        "hr" | "hrv" => "Croatian",
        "hu" | "hun" => "Hungarian",
        "id" | "ind" => "Indonesian",
        "it" | "ita" => "Italian",
        "ja" | "jpn" => "Japanese",
        "ko" | "kor" => "Korean",
        "lt" | "lit" => "Lithuanian",
        "lv" | "lav" => "Latvian",
        "nl" | "nld" | "dut" => "Dutch",
        "no" | "nor" => "Norwegian",
        "pl" | "pol" => "Polish",
        "pt" | "por" => "Portuguese",
        "ro" | "ron" | "rum" => "Romanian",
        "ru" | "rus" => "Russian",
        "sk" | "slk" | "slo" => "Slovak",
        "sl" | "slv" => "Slovenian",
        "sr" | "srp" => "Serbian",
        "sv" | "swe" => "Swedish",
        "th" | "tha" => "Thai",
        "tr" | "tur" => "Turkish",
        "uk" | "ukr" => "Ukrainian",
        "vi" | "vie" => "Vietnamese",
        "zh" | "zho" | "chi" => "Chinese",
        "und" | "unknown" => "Unknown",
        _ if raw.chars().count() > 3 => raw,
        _ => return raw.to_ascii_uppercase(),
    }
    .to_owned()
}

fn send_or_show(
    controller: &PlaybackController,
    command: PlaybackCommand,
    ui: &slint::Weak<MainWindow>,
) {
    if let Err(error) = controller.send(command) {
        show_player_error(ui, error.to_string());
    }
}

fn log_command(result: Result<(), playback_mpv::MpvError>) {
    if let Err(error) = result {
        tracing::error!(%error, "MPV command failed");
    }
}

fn show_player_error(ui: &slint::Weak<MainWindow>, message: String) {
    tracing::error!(error = %message, "player error shown in UI");
    let ui = ui.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(ui) = ui.upgrade() {
            ui.set_player_loading(false);
            ui.set_player_buffering(false);
            ui.set_player_has_video_frame(false);
            ui.set_player_error(message.into());
        }
    });
}

fn lock_session(session: &Mutex<SessionState>) -> std::sync::MutexGuard<'_, SessionState> {
    session
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn read_state(state: &RwLock<PlaybackState>) -> std::sync::RwLockReadGuard<'_, PlaybackState> {
    state
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_statistics_poll(
    poll: &Mutex<Option<StatisticsPoll>>,
) -> std::sync::MutexGuard<'_, Option<StatisticsPoll>> {
    poll.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn cancel_statistics_poll(poll: &Mutex<Option<StatisticsPoll>>) {
    if let Some(poll) = lock_statistics_poll(poll).take() {
        poll.cancellation.cancel();
    }
}

fn unload_player(
    controller: &PlaybackController,
    core: &Arc<Runtime<DesktopEnv, AppModel>>,
    statistics_poll: &Mutex<Option<StatisticsPoll>>,
    session: &Mutex<SessionState>,
) {
    log_command(controller.send(PlaybackCommand::Stop));
    cancel_statistics_poll(statistics_poll);
    *lock_session(session) = SessionState::default();
    core.dispatch(RuntimeAction {
        field: Some(AppModelField::Player),
        action: Action::Unload,
    });
}
