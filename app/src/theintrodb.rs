use std::{sync::OnceLock, time::Duration};

use anyhow::Context;

#[derive(Clone, Debug, PartialEq)]
pub struct TidbSegment {
    pub segment_type: String,
    pub start_secs: f64,
    pub end_secs: f64,
}

#[derive(serde::Deserialize)]
struct TidbResponseSegment {
    start_ms: Option<f64>,
    end_ms: Option<f64>,
}

#[derive(serde::Deserialize)]
struct TidbResponse {
    intro: Option<Vec<TidbResponseSegment>>,
    recap: Option<Vec<TidbResponseSegment>>,
    credits: Option<Vec<TidbResponseSegment>>,
    preview: Option<Vec<TidbResponseSegment>>,
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "could not configure TheIntroDB HTTP client; using defaults");
                reqwest::Client::new()
            })
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the TheIntroDB query parameters one-to-one; a request struct would be built and destructured at a single call site"
)]
pub fn fetch_segments(
    runtime_handle: &tokio::runtime::Handle,
    api_key: String,
    id_type: &'static str,
    media_id: String,
    season: Option<u32>,
    episode: Option<u32>,
    duration_secs: i64,
    on_complete: impl FnOnce(Vec<TidbSegment>) + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    runtime_handle.spawn(async move {
        let segments =
            request_segments(&api_key, id_type, &media_id, season, episode, duration_secs)
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, "TheIntroDB request failed");
                    Vec::new()
                });
        on_complete(segments);
    })
}

async fn request_segments(
    api_key: &str,
    id_type: &str,
    media_id: &str,
    season: Option<u32>,
    episode: Option<u32>,
    duration_secs: i64,
) -> anyhow::Result<Vec<TidbSegment>> {
    let mut url = url::Url::parse("https://api.theintrodb.org/v3/media")
        .context("invalid TheIntroDB endpoint")?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("duration_ms", &(duration_secs * 1_000).to_string());
        query.append_pair(id_type, media_id);
        if let Some(season) = season {
            query.append_pair("season", &season.to_string());
        }
        if let Some(episode) = episode {
            query.append_pair("episode", &episode.to_string());
        }
    }

    tracing::info!(%url, "fetching TheIntroDB segments");
    let mut request = client()
        .get(url)
        .header("User-Agent", "TheIntroDB Stremio Native Client");
    if !api_key.is_empty() {
        request = request.bearer_auth(api_key);
    }
    let response = request
        .send()
        .await
        .context("could not reach TheIntroDB")?
        .error_for_status()
        .context("TheIntroDB returned an error status")?;
    let response = response
        .json::<TidbResponse>()
        .await
        .context("could not decode TheIntroDB response")?;

    let mut segments = Vec::new();
    append_segments(&mut segments, response.intro, "intro", duration_secs);
    append_segments(&mut segments, response.recap, "recap", duration_secs);
    append_segments(&mut segments, response.credits, "credits", duration_secs);
    append_segments(&mut segments, response.preview, "preview", duration_secs);
    tracing::info!(count = segments.len(), "loaded TheIntroDB segments");
    Ok(segments)
}

fn append_segments(
    segments: &mut Vec<TidbSegment>,
    source: Option<Vec<TidbResponseSegment>>,
    segment_type: &str,
    duration_secs: i64,
) {
    let Some(source) = source else {
        return;
    };
    segments.extend(source.into_iter().map(|segment| {
        TidbSegment {
            segment_type: segment_type.to_owned(),
            start_secs: segment.start_ms.unwrap_or_default() / 1_000.0,
            end_secs: segment
                .end_ms
                .map(|milliseconds| milliseconds / 1_000.0)
                .unwrap_or(duration_secs as f64),
        }
    }));
}

pub fn check_active_segment(current_time: f64, segments: &[TidbSegment]) -> Option<&TidbSegment> {
    segments
        .iter()
        .find(|segment| current_time >= segment.start_secs && current_time < segment.end_secs)
}

#[cfg(test)]
mod tests {
    use super::{TidbSegment, check_active_segment};

    #[test]
    fn active_segment_uses_inclusive_start_and_exclusive_end() {
        let segments = [TidbSegment {
            segment_type: "intro".to_owned(),
            start_secs: 10.0,
            end_secs: 20.0,
        }];

        assert_eq!(
            check_active_segment(10.0, &segments).map(|segment| segment.segment_type.as_str()),
            Some("intro")
        );
        assert!(check_active_segment(20.0, &segments).is_none());
    }
}
