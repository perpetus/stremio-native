use futures::StreamExt;
use moka::sync::Cache;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use tokio::sync::Semaphore;
use url::Url;

type ImageEntry = SharedPixelBuffer<Rgba8Pixel>;
type RefreshFn = Arc<dyn Fn(Vec<String>) + Send + Sync>;

#[derive(Clone)]
enum DownloadState {
    InProgress,
    Failed { attempts: u32, retry_at: Instant },
}

static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static IMAGE_CACHE: OnceLock<Cache<String, ImageEntry>> = OnceLock::new();
static REQUIRED_IMAGE_CACHE: OnceLock<Cache<String, ImageEntry>> = OnceLock::new();
static DOWNLOAD_STATE: OnceLock<Cache<String, DownloadState>> = OnceLock::new();
static FETCH_QUEUE: OnceLock<Mutex<FetchQueue>> = OnceLock::new();
static NETWORK_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static DISK_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static DECODE_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static REFRESH_CALLBACK: OnceLock<RefreshFn> = OnceLock::new();
static REFRESH_URLS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static REFRESH_PENDING: AtomicBool = AtomicBool::new(false);
static MAINTENANCE_STARTED: AtomicBool = AtomicBool::new(false);

const MEMORY_CAPACITY_BYTES: u64 = 32 * 1024 * 1024;
const REQUIRED_IMAGE_IDLE_TTL: Duration = Duration::from_secs(60);
const DISK_CAPACITY_BYTES: u64 = 1536 * 1024 * 1024;
const DISK_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_DECODE_WIDTH: u32 = 1920;
const MAX_DECODE_HEIGHT: u32 = 1920;
const MAX_CONCURRENT_DOWNLOADS: usize = 8;
const MAX_CONCURRENT_DISK_READS: usize = 4;
const MAX_CONCURRENT_DECODES: usize = 3;
const MAX_FETCH_WORKERS: usize = 16;
const MAX_RETRY_ATTEMPTS: u32 = 3;

struct FetchJob {
    url: String,
    previous_attempts: u32,
}

#[derive(Default)]
struct FetchQueue {
    jobs: VecDeque<FetchJob>,
    active_workers: usize,
}

pub fn set_refresh_callback(f: impl Fn(Vec<String>) + Send + Sync + 'static) {
    let _ = REFRESH_CALLBACK.set(Arc::new(f));
}

pub fn get_poster_image(
    url: &Option<Url>,
    _ui_weak: &slint::Weak<crate::MainWindow>,
) -> slint::Image {
    get_poster_image_ref(url.as_ref(), _ui_weak)
}

pub fn get_poster_image_ref(
    url: Option<&Url>,
    _ui_weak: &slint::Weak<crate::MainWindow>,
) -> slint::Image {
    let Some(url) = url else {
        return slint::Image::default();
    };

    get_image(url.as_str(), true)
}

/// Returns an already-decoded image without initiating I/O. This is used while
/// building virtualized models; the visible Slint delegates request misses.
pub fn get_cached_image(url: &Option<Url>) -> slint::Image {
    let Some(url) = url else {
        return slint::Image::default();
    };

    get_image(url.as_str(), false)
}

pub fn get_cached_image_url(url: &str) -> slint::Image {
    if url.is_empty() {
        slint::Image::default()
    } else {
        get_image(url, false)
    }
}

pub fn request_image(url: &str) {
    if !url.is_empty() {
        let _ = get_image(url, true);
    }
}

fn get_image(url: &str, request_on_miss: bool) -> slint::Image {
    if let Some(buffer) = required_image_cache().get(url) {
        crate::performance::counters().record_image_memory_hit();
        return slint::Image::from_rgba8(buffer);
    }
    if let Some(buffer) = image_cache().get(url) {
        crate::performance::counters().record_image_memory_hit();
        if request_on_miss {
            required_image_cache().insert(url.to_owned(), buffer.clone());
        }
        return slint::Image::from_rgba8(buffer);
    }

    if request_on_miss {
        schedule_fetch(url.to_owned());
    }

    slint::Image::default()
}

/// Holds the currently requested decoded working set. It starts empty and
/// grows beyond the 32 MiB base cache only while images continue to be used.
fn required_image_cache() -> &'static Cache<String, ImageEntry> {
    REQUIRED_IMAGE_CACHE.get_or_init(|| {
        Cache::builder()
            .time_to_idle(REQUIRED_IMAGE_IDLE_TTL)
            .build()
    })
}

fn image_cache() -> &'static Cache<String, ImageEntry> {
    IMAGE_CACHE.get_or_init(|| {
        Cache::builder()
            .max_capacity(MEMORY_CAPACITY_BYTES)
            .weigher(|_key: &String, value: &ImageEntry| {
                value
                    .width()
                    .saturating_mul(value.height())
                    .saturating_mul(4)
            })
            .time_to_idle(Duration::from_secs(10 * 60))
            .build()
    })
}

fn download_state() -> &'static Cache<String, DownloadState> {
    DOWNLOAD_STATE.get_or_init(|| {
        Cache::builder()
            .max_capacity(10_000)
            .time_to_idle(Duration::from_secs(30 * 60))
            .build()
    })
}

fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "Origin",
            "https://app.strem.io".parse().expect("valid Origin"),
        );
        headers.insert(
            "Referer",
            "https://app.strem.io/".parse().expect("valid Referer"),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            "image/avif,image/webp,image/apng,image/*,*/*;q=0.8"
                .parse()
                .expect("valid Accept"),
        );

        reqwest::Client::builder()
            .user_agent("Stremio-Rust/0.1")
            .default_headers(headers)
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .pool_max_idle_per_host(4)
            .build()
            .unwrap_or_default()
    })
}

fn network_semaphore() -> &'static Arc<Semaphore> {
    NETWORK_SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOADS)))
}

fn disk_semaphore() -> &'static Arc<Semaphore> {
    DISK_SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_DISK_READS)))
}

fn decode_semaphore() -> &'static Arc<Semaphore> {
    DECODE_SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_DECODES)))
}

fn schedule_fetch(url: String) {
    let now = Instant::now();
    let previous_attempts = match download_state().get(&url) {
        Some(DownloadState::InProgress) => return,
        Some(DownloadState::Failed { retry_at, .. }) if retry_at > now => return,
        Some(DownloadState::Failed { attempts, .. }) => attempts,
        None => 0,
    };

    download_state().insert(url.clone(), DownloadState::InProgress);
    start_disk_maintenance_once();

    let workers_to_start = {
        let mut queue = fetch_queue()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        queue.jobs.push_back(FetchJob {
            url,
            previous_attempts,
        });
        let available_workers = MAX_FETCH_WORKERS.saturating_sub(queue.active_workers);
        let workers_to_start = available_workers.min(queue.jobs.len());
        queue.active_workers += workers_to_start;
        workers_to_start
    };
    for _ in 0..workers_to_start {
        tokio::spawn(fetch_worker());
    }
}

fn fetch_queue() -> &'static Mutex<FetchQueue> {
    FETCH_QUEUE.get_or_init(|| Mutex::new(FetchQueue::default()))
}

async fn fetch_worker() {
    loop {
        let job = {
            let mut queue = fetch_queue()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match queue.jobs.pop_front() {
                Some(job) => job,
                None => {
                    queue.active_workers = queue.active_workers.saturating_sub(1);
                    return;
                }
            }
        };
        process_fetch(job).await;
    }
}

async fn process_fetch(job: FetchJob) {
    let result = match load_from_disk(&job.url).await {
        Some(buffer) => {
            crate::performance::counters().record_image_disk_hit();
            Ok(buffer)
        }
        None => download_and_cache(&job.url).await,
    };

    match result {
        Ok(buffer) => {
            required_image_cache().insert(job.url.clone(), buffer.clone());
            image_cache().insert(job.url.clone(), buffer);
            download_state().invalidate(&job.url);
            notify_ui_refresh(job.url);
        }
        Err(error) => {
            crate::performance::counters().record_image_failure();
            let attempts = job.previous_attempts.saturating_add(1);
            let backoff_seconds = 2_u64.saturating_pow(attempts.min(6)).max(2);
            download_state().insert(
                job.url.clone(),
                DownloadState::Failed {
                    attempts,
                    retry_at: Instant::now() + Duration::from_secs(backoff_seconds),
                },
            );
            tracing::warn!(url = %job.url, %error, attempts, "image request failed");
        }
    }
}

#[tracing::instrument(skip_all, fields(url = %url))]
async fn load_from_disk(url: &str) -> Option<ImageEntry> {
    let _permit = disk_semaphore().clone().acquire_owned().await.ok()?;
    let path = cache_path(url);
    let bytes = tokio::task::spawn_blocking(move || {
        let metadata = std::fs::metadata(&path).ok()?;
        let age = metadata.modified().ok()?.elapsed().ok()?;
        if age > DISK_TTL || metadata.len() > MAX_RESPONSE_BYTES as u64 {
            let _ = std::fs::remove_file(path);
            return None;
        }
        std::fs::read(path).ok()
    })
    .await
    .ok()
    .flatten()?;

    decode(bytes).await.ok().map(|(decoded, _bytes)| decoded)
}

#[tracing::instrument(skip_all, fields(url = %url))]
async fn download_and_cache(url: &str) -> Result<ImageEntry, String> {
    let _permit = network_semaphore()
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| "image network queue closed".to_owned())?;
    let bytes = download_with_retry(url).await?;
    crate::performance::counters().record_image_download();

    let (decoded, bytes) = decode(bytes).await?;
    let path = cache_path(url);
    match tokio::task::spawn_blocking(move || write_cache_file(&path, &bytes)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(%error, "image cache write failed"),
        Err(error) => tracing::warn!(%error, "image cache writer stopped"),
    }
    Ok(decoded)
}

async fn download_with_retry(url: &str) -> Result<Vec<u8>, String> {
    let mut last_error = "request was not attempted".to_owned();

    for attempt in 0..MAX_RETRY_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(250 * 2_u64.pow(attempt - 1))).await;
        }

        let response = match client().get(url).send().await {
            Ok(response) => response,
            Err(error) => {
                last_error = error.to_string();
                continue;
            }
        };

        if !response.status().is_success() {
            last_error = format!("HTTP {}", response.status());
            if response.status().is_client_error() {
                break;
            }
            continue;
        }

        if response
            .content_length()
            .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
        {
            return Err(format!("image exceeds {MAX_RESPONSE_BYTES} byte limit"));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if content_type.contains("text/html") || content_type.contains("svg") {
            return Err(format!("unsupported image content type: {content_type}"));
        }

        let mut body = Vec::with_capacity(response.content_length().unwrap_or(0) as usize);
        let mut stream = response.bytes_stream();
        let mut failed = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) if body.len().saturating_add(chunk.len()) <= MAX_RESPONSE_BYTES => {
                    body.extend_from_slice(&chunk);
                }
                Ok(_) => {
                    failed = Some(format!("image exceeds {MAX_RESPONSE_BYTES} byte limit"));
                    break;
                }
                Err(error) => {
                    failed = Some(error.to_string());
                    break;
                }
            }
        }
        if let Some(error) = failed {
            last_error = error;
            continue;
        }
        if !body.is_empty() {
            return Ok(body);
        }
        last_error = "empty image response".to_owned();
    }

    Err(last_error)
}

#[tracing::instrument(skip_all, fields(size = bytes.len()))]
async fn decode(bytes: Vec<u8>) -> Result<(ImageEntry, Vec<u8>), String> {
    let _permit = decode_semaphore()
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| "image decode queue closed".to_owned())?;
    tokio::task::spawn_blocking(move || {
        let image = image::load_from_memory(&bytes).map_err(|error| error.to_string())?;
        let image = if image.width() > MAX_DECODE_WIDTH || image.height() > MAX_DECODE_HEIGHT {
            image.resize(
                MAX_DECODE_WIDTH,
                MAX_DECODE_HEIGHT,
                image::imageops::FilterType::Triangle,
            )
        } else {
            image
        };
        let rgba = image.into_rgba8();
        let decoded = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
        );
        Ok((decoded, bytes))
    })
    .await
    .map_err(|error| format!("image decoder stopped: {error}"))?
}

fn cache_root() -> PathBuf {
    PathBuf::from("storage").join("image-cache-v1")
}

fn cache_path(url: &str) -> PathBuf {
    let hash = blake3::hash(url.as_bytes()).to_hex().to_string();
    cache_root().join(&hash[..2]).join(format!("{hash}.img"))
}

fn write_cache_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    match std::fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() => {
            let _ = std::fs::remove_file(temporary);
            Ok(())
        }
        Err(error) => {
            let _ = std::fs::remove_file(temporary);
            Err(error)
        }
    }
}

fn start_disk_maintenance_once() {
    if MAINTENANCE_STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    tokio::task::spawn_blocking(|| {
        if let Err(error) = maintain_disk_cache() {
            tracing::warn!(%error, "image cache maintenance failed");
        }
    });
}

fn maintain_disk_cache() -> std::io::Result<()> {
    let root = cache_root();
    std::fs::create_dir_all(&root)?;
    let mut files = Vec::new();
    let now = SystemTime::now();

    for shard in std::fs::read_dir(&root)?.flatten() {
        if !shard.path().is_dir() {
            continue;
        }
        for file in std::fs::read_dir(shard.path())?.flatten() {
            let path = file.path();
            let Ok(metadata) = file.metadata() else {
                continue;
            };
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            if now.duration_since(modified).unwrap_or_default() > DISK_TTL {
                let _ = std::fs::remove_file(path);
            } else {
                files.push((modified, metadata.len(), path));
            }
        }
    }

    let mut total = files.iter().map(|(_, size, _)| *size).sum::<u64>();
    if total <= DISK_CAPACITY_BYTES {
        return Ok(());
    }
    files.sort_unstable_by_key(|(modified, _, _)| *modified);
    for (_, size, path) in files {
        if total <= DISK_CAPACITY_BYTES {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
    Ok(())
}

fn notify_ui_refresh(url: String) {
    REFRESH_URLS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(url);

    if REFRESH_PENDING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }

    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(16)).await;
        dispatch_ui_refresh();
    });
}

fn dispatch_ui_refresh() {
    let urls = drain_refresh_urls();
    if urls.is_empty() {
        finish_ui_refresh();
        return;
    }
    let Some(callback) = REFRESH_CALLBACK.get().cloned() else {
        finish_ui_refresh();
        return;
    };
    let result = slint::invoke_from_event_loop(move || {
        callback(urls);
        finish_ui_refresh();
    });
    if let Err(error) = result {
        tracing::error!(%error, "could not enqueue image refresh on the Slint event loop");
        finish_ui_refresh();
    }
}

fn drain_refresh_urls() -> Vec<String> {
    REFRESH_URLS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .drain()
        .collect()
}

fn finish_ui_refresh() {
    REFRESH_PENDING.store(false, Ordering::Release);
    let has_pending_urls = !REFRESH_URLS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_empty();
    if has_pending_urls
        && REFRESH_PENDING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    {
        dispatch_ui_refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_RESPONSE_BYTES, MEMORY_CAPACITY_BYTES, REQUIRED_IMAGE_IDLE_TTL, cache_path};

    #[test]
    fn cache_path_is_stable_and_sharded() {
        let first = cache_path("https://example.com/poster.jpg");
        let second = cache_path("https://example.com/poster.jpg");
        assert_eq!(first, second);
        assert!(first.parent().and_then(|path| path.file_name()).is_some());
    }

    #[test]
    fn response_limit_is_not_unbounded() {
        assert!(MAX_RESPONSE_BYTES <= 32 * 1024 * 1024);
    }

    #[test]
    fn decoded_cache_has_a_small_base_and_temporary_growth_window() {
        assert_eq!(MEMORY_CAPACITY_BYTES, 32 * 1024 * 1024);
        assert!(!REQUIRED_IMAGE_IDLE_TTL.is_zero());
    }
}
