use std::sync::Arc;

use core_env::DesktopEnv;
use percent_encoding::percent_decode_str;
use stremio_core::{
    models::{
        calendar::YearMonthDate,
        catalog_with_filters::Selected as CatalogSelected,
        library_with_filters::{LibraryRequest, Selected as LibrarySelected, Sort as LibrarySort},
        player::Selected as PlayerSelected,
    },
    runtime::{
        Runtime, RuntimeAction,
        msg::{Action, ActionLoad},
    },
    types::{
        addon::{ExtraValue, ResourcePath, ResourceRequest},
        resource::{Stream, StreamBehaviorHints, StreamSource},
    },
};
use url::Url;

use crate::{
    AppModel, MainWindow, NavigationController, NavigationIntent, Tab,
    models::details::{load_meta_details_for_video, open_details_route},
    single_instance::AppCommand,
};

enum DeepLink {
    Activate,
    Tab(Tab),
    Details {
        media_type: String,
        media_id: String,
        video_id: Option<String>,
    },
    Search(String),
    Discover(ResourceRequest),
    Library(LibraryRequest),
    Calendar(YearMonthDate),
    AddonDetails(Url),
    Player(PlayerDeepLink),
    Error(String),
}

struct PlayerDeepLink {
    selected: PlayerSelected,
    media_type: Option<String>,
    media_id: Option<String>,
    video_id: Option<String>,
}

pub fn start_command_receiver(
    mut commands: tokio::sync::mpsc::UnboundedReceiver<AppCommand>,
    ui_weak: slint::Weak<MainWindow>,
    runtime: Arc<Runtime<DesktopEnv, AppModel>>,
    navigation: NavigationController,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(command) = commands.recv().await {
            let ui_weak = ui_weak.clone();
            let runtime = runtime.clone();
            let navigation = navigation.clone();
            if let Err(error) = slint::invoke_from_event_loop(move || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                handle(command, &ui, &runtime, &navigation);
            }) {
                tracing::warn!(%error, "deep-link command arrived after the UI stopped");
                break;
            }
        }
    })
}

pub fn handle(
    command: AppCommand,
    ui: &MainWindow,
    runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
    navigation: &NavigationController,
) {
    let deep_link = match parse_command(command) {
        Ok(deep_link) => deep_link,
        Err(error) => {
            tracing::warn!(%error, "unsupported or malformed deep link");
            ui.set_error_message(format!("Could not open link: {error}").into());
            show_window(ui);
            return;
        }
    };

    match deep_link {
        DeepLink::Activate => show_window(ui),
        DeepLink::Tab(tab) => {
            ui.invoke_tab_changed(tab.index());
            show_window(ui);
        }
        DeepLink::Details {
            media_type,
            media_id,
            video_id,
        } => {
            open_details(ui, runtime, navigation, media_type, media_id, video_id);
            show_window(ui);
        }
        DeepLink::Search(query) => {
            ui.invoke_global_search_submitted(query.into());
            show_window(ui);
        }
        DeepLink::Discover(request) => {
            ui.invoke_tab_changed(Tab::Discover.index());
            crate::models::discover::clear_sync_state();
            runtime.dispatch(RuntimeAction {
                field: None,
                action: Action::Load(ActionLoad::CatalogWithFilters(Some(CatalogSelected {
                    request,
                }))),
            });
            show_window(ui);
        }
        DeepLink::Library(request) => {
            ui.invoke_tab_changed(Tab::Library.index());
            crate::models::library::clear_sync_state();
            runtime.dispatch(RuntimeAction {
                field: None,
                action: Action::Load(ActionLoad::LibraryWithFilters(LibrarySelected { request })),
            });
            show_window(ui);
        }
        DeepLink::Calendar(selected) => {
            ui.invoke_tab_changed(Tab::Calendar.index());
            ui.set_calendar_loading(true);
            runtime.dispatch(RuntimeAction {
                field: None,
                action: Action::Load(ActionLoad::Calendar(Some(selected))),
            });
            show_window(ui);
        }
        DeepLink::AddonDetails(transport_url) => {
            ui.invoke_tab_changed(Tab::Addons.index());
            ui.invoke_open_addon_details(transport_url.as_str().into());
            show_window(ui);
        }
        DeepLink::Player(player) => {
            open_player(ui, runtime, navigation, player);
            show_window(ui);
        }
        DeepLink::Error(message) => {
            ui.set_error_message(message.into());
            show_window(ui);
        }
    }
}

fn parse_command(command: AppCommand) -> anyhow::Result<DeepLink> {
    let AppCommand::Open(value) = command else {
        return Ok(DeepLink::Activate);
    };
    let url = Url::parse(&value)?;
    match url.scheme() {
        "magnet" => parse_magnet(url),
        "stremio" => parse_stremio(url),
        scheme => Err(anyhow::anyhow!("unsupported URL scheme {scheme:?}")),
    }
}

fn parse_stremio(url: Url) -> anyhow::Result<DeepLink> {
    if let Some(host) = url.host_str() {
        if !is_navigation_root(host) {
            let transport_url = Url::parse(&url.as_str().replacen("stremio://", "https://", 1))?;
            return Ok(DeepLink::AddonDetails(transport_url));
        }
    }

    let mut segments = url
        .path_segments()
        .ok_or_else(|| anyhow::anyhow!("deep link has no route"))?
        .map(decode_segment)
        .collect::<anyhow::Result<Vec<_>>>()?;
    if let Some(host) = url.host_str().filter(|host| is_navigation_root(host)) {
        segments.insert(0, host.to_owned());
    }
    let route = segments.first().map(String::as_str).unwrap_or_default();

    match route {
        "" | "board" => Ok(DeepLink::Tab(Tab::Board)),
        "settings" => Ok(DeepLink::Tab(Tab::Settings)),
        "detail" => parse_details(&segments),
        "player" => parse_player(&segments),
        "search" => {
            let query = query_value(&url, "query").unwrap_or_default();
            if query.trim().is_empty() {
                Err(anyhow::anyhow!("search link has no query"))
            } else {
                Ok(DeepLink::Search(query))
            }
        }
        "discover" => parse_discover(&url, &segments),
        "library" => parse_library(&url, &segments),
        "continuewatching" => Ok(DeepLink::Tab(Tab::Board)),
        "calendar" => parse_calendar(&segments),
        "addons" => parse_addons(&segments),
        "error" => Ok(DeepLink::Error(
            query_value(&url, "message").unwrap_or_else(|| "The link reported an error".to_owned()),
        )),
        unknown => Err(anyhow::anyhow!("unsupported Stremio route {unknown:?}")),
    }
}

fn parse_details(segments: &[String]) -> anyhow::Result<DeepLink> {
    let media_type = required_segment(segments, 1, "media type")?.to_owned();
    let media_id = required_segment(segments, 2, "media id")?.to_owned();
    let video_id = segments.get(3).filter(|value| !value.is_empty()).cloned();
    Ok(DeepLink::Details {
        media_type,
        media_id,
        video_id,
    })
}

fn parse_player(segments: &[String]) -> anyhow::Result<DeepLink> {
    let encoded_stream = required_segment(segments, 1, "encoded stream")?;
    let stream = Stream::decode(encoded_stream)
        .map_err(|error| anyhow::anyhow!("invalid player stream: {error}"))?;

    let request_parts = match (
        segments.get(2),
        segments.get(3),
        segments.get(4),
        segments.get(5),
        segments.get(6),
    ) {
        (Some(stream_base), Some(meta_base), Some(media_type), Some(media_id), Some(video_id)) => {
            Some((
                Url::parse(stream_base)?,
                Url::parse(meta_base)?,
                media_type.clone(),
                media_id.clone(),
                video_id.clone(),
            ))
        }
        _ => None,
    };

    let (stream_request, meta_request, subtitles_path, media_type, media_id, video_id) =
        if let Some((stream_base, meta_base, media_type, media_id, video_id)) = request_parts {
            (
                Some(ResourceRequest::new(
                    stream_base,
                    ResourcePath::without_extra("stream", &media_type, &video_id),
                )),
                Some(ResourceRequest::new(
                    meta_base,
                    ResourcePath::without_extra("meta", &media_type, &media_id),
                )),
                Some(ResourcePath::without_extra(
                    "subtitles",
                    &media_type,
                    &video_id,
                )),
                Some(media_type),
                Some(media_id),
                Some(video_id),
            )
        } else {
            (None, None, None, None, None, None)
        };

    Ok(DeepLink::Player(PlayerDeepLink {
        selected: PlayerSelected {
            stream,
            stream_request,
            meta_request,
            subtitles_path,
        },
        media_type,
        media_id,
        video_id,
    }))
}

fn parse_magnet(url: Url) -> anyhow::Result<DeepLink> {
    let name = url
        .query_pairs()
        .find_map(|(name, value)| (name == "dn").then(|| value.into_owned()));
    Ok(DeepLink::Player(PlayerDeepLink {
        selected: PlayerSelected {
            stream: Stream {
                source: StreamSource::Url { url },
                name: name.clone(),
                description: None,
                thumbnail: None,
                subtitles: Vec::new(),
                behavior_hints: StreamBehaviorHints::default(),
            },
            stream_request: None,
            meta_request: None,
            subtitles_path: None,
        },
        media_type: None,
        media_id: None,
        video_id: None,
    }))
}

fn parse_discover(url: &Url, segments: &[String]) -> anyhow::Result<DeepLink> {
    if segments.len() == 1 {
        return Ok(DeepLink::Tab(Tab::Discover));
    }
    let base = Url::parse(required_segment(segments, 1, "catalog transport URL")?)?;
    let media_type = required_segment(segments, 2, "catalog media type")?;
    let catalog_id = required_segment(segments, 3, "catalog id")?;
    let extra = url
        .query_pairs()
        .map(|(name, value)| ExtraValue {
            name: name.into_owned(),
            value: value.into_owned(),
        })
        .collect();
    Ok(DeepLink::Discover(ResourceRequest::new(
        base,
        ResourcePath {
            resource: "catalog".to_owned(),
            r#type: media_type.to_owned(),
            id: catalog_id.to_owned(),
            extra,
        },
    )))
}

fn parse_library(url: &Url, segments: &[String]) -> anyhow::Result<DeepLink> {
    let media_type = segments
        .get(1)
        .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("all"))
        .cloned();
    let sort = query_value(url, "sort")
        .as_deref()
        .map(parse_library_sort)
        .unwrap_or_default();
    Ok(DeepLink::Library(LibraryRequest {
        r#type: media_type,
        sort,
        page: Default::default(),
    }))
}

fn parse_calendar(segments: &[String]) -> anyhow::Result<DeepLink> {
    if segments.len() == 1 {
        return Ok(DeepLink::Tab(Tab::Calendar));
    }
    let year = required_segment(segments, 1, "calendar year")?.parse::<i32>()?;
    let month = required_segment(segments, 2, "calendar month")?.parse::<u32>()?;
    if !(1..=12).contains(&month) {
        return Err(anyhow::anyhow!("calendar month must be between 1 and 12"));
    }
    Ok(DeepLink::Calendar(YearMonthDate { month, year }))
}

fn parse_addons(segments: &[String]) -> anyhow::Result<DeepLink> {
    match segments.get(2).filter(|value| !value.is_empty()) {
        Some(transport_url) => Ok(DeepLink::AddonDetails(Url::parse(transport_url)?)),
        None => Ok(DeepLink::Tab(Tab::Addons)),
    }
}

fn open_details(
    ui: &MainWindow,
    runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
    navigation: &NavigationController,
    media_type: String,
    media_id: String,
    video_id: Option<String>,
) {
    open_details_route(ui, runtime, navigation, &media_id);
    load_meta_details_for_video(runtime, media_id, Some(media_type), video_id);
}

fn open_player(
    ui: &MainWindow,
    runtime: &Arc<Runtime<DesktopEnv, AppModel>>,
    navigation: &NavigationController,
    player: PlayerDeepLink,
) {
    if let (Some(media_type), Some(media_id)) = (&player.media_type, &player.media_id) {
        open_details(
            ui,
            runtime,
            navigation,
            media_type.clone(),
            media_id.clone(),
            player.video_id.clone(),
        );
    }

    let title = player
        .selected
        .stream
        .name
        .as_deref()
        .or(player.media_id.as_deref())
        .unwrap_or("Stremio stream");
    let stream_name = player
        .selected
        .stream
        .description
        .as_deref()
        .unwrap_or(title);
    ui.set_player_title(title.into());
    ui.set_player_stream_name(stream_name.into());
    ui.set_player_error("".into());
    ui.set_player_loading(true);
    ui.set_player_buffering(false);
    ui.set_player_buffering_percent(0.0);
    ui.set_player_controls_visible(true);
    ui.set_player_is_series(player.media_type.as_deref() == Some("series"));
    ui.set_player_seasons(Default::default());
    ui.set_player_episodes(Default::default());
    ui.set_player_active_video_id(player.video_id.as_deref().unwrap_or_default().into());
    ui.set_player_active_episode_idx(0);
    ui.set_player_has_next_episode(false);
    navigation.dispatch_and_project(ui, NavigationIntent::OpenPlayer);
    runtime.dispatch(RuntimeAction {
        field: None,
        action: Action::Load(ActionLoad::Player(Box::new(player.selected))),
    });
}

fn show_window(ui: &MainWindow) {
    crate::tray::show_window(ui);
}

fn required_segment<'a>(
    segments: &'a [String],
    index: usize,
    description: &str,
) -> anyhow::Result<&'a str> {
    segments
        .get(index)
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .ok_or_else(|| anyhow::anyhow!("deep link has no {description}"))
}

fn decode_segment(value: &str) -> anyhow::Result<String> {
    percent_decode_str(value)
        .decode_utf8()
        .map(|value| value.into_owned())
        .map_err(|error| anyhow::anyhow!("deep-link path is not UTF-8: {error}"))
}

fn query_value(url: &Url, expected: &str) -> Option<String> {
    url.query_pairs()
        .find_map(|(name, value)| (name == expected).then(|| value.into_owned()))
}

fn parse_library_sort(value: &str) -> LibrarySort {
    match value.to_ascii_lowercase().as_str() {
        "name" => LibrarySort::Name,
        "namereverse" => LibrarySort::NameReverse,
        "timeswatched" => LibrarySort::TimesWatched,
        "watched" => LibrarySort::Watched,
        "notwatched" => LibrarySort::NotWatched,
        _ => LibrarySort::LastWatched,
    }
}

fn is_navigation_root(value: &str) -> bool {
    matches!(
        value,
        "board"
            | "settings"
            | "detail"
            | "player"
            | "search"
            | "discover"
            | "library"
            | "continuewatching"
            | "calendar"
            | "addons"
            | "error"
    )
}

#[cfg(test)]
mod tests {
    use super::{DeepLink, parse_command};
    use crate::single_instance::AppCommand;

    #[test]
    fn parses_official_details_link() -> anyhow::Result<()> {
        let link = parse_command(AppCommand::Open(
            "stremio:///detail/series/tt13622776/tt13622776%3A1%3A5".to_owned(),
        ))?;
        assert!(matches!(
            link,
            DeepLink::Details {
                media_type,
                media_id,
                video_id: Some(video_id),
            } if media_type == "series"
                && media_id == "tt13622776"
                && video_id == "tt13622776:1:5"
        ));
        Ok(())
    }

    #[test]
    fn parses_official_search_link() -> anyhow::Result<()> {
        let link = parse_command(AppCommand::Open(
            "stremio:///search?query=better%20call%20saul".to_owned(),
        ))?;
        assert!(matches!(link, DeepLink::Search(query) if query == "better call saul"));
        Ok(())
    }
}
