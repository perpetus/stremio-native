use chrono::{DateTime, Utc};
use futures::{FutureExt, future};
use http::Request;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::runtime::Handle;

use stremio_core::{
    models::{ctx::Ctx, streaming_server::StreamingServer},
    runtime::{Env, EnvError, EnvFuture, TryEnvFuture},
};

static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
static TOKIO_HANDLE: OnceLock<Handle> = OnceLock::new();

/// Registers the application runtime so core work can be scheduled safely from
/// native callback threads such as the libmpv actor.
pub fn install_runtime_handle(handle: Handle) {
    let _ = TOKIO_HANDLE.set(handle);
}

fn spawn_on_runtime(future: impl Future<Output = ()> + Send + 'static) {
    if let Some(handle) = TOKIO_HANDLE.get() {
        drop(handle.spawn(future));
    } else if let Ok(handle) = Handle::try_current() {
        drop(handle.spawn(future));
    } else {
        tracing::error!("cannot schedule core future because no Tokio runtime is registered");
    }
}

fn get_http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Origin", "https://app.strem.io".parse().unwrap());
        headers.insert("Referer", "https://app.strem.io/".parse().unwrap());

        reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Stremio/4.4.168 Chrome/110.0.0.0 Safari/537.36")
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(15))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client")
    })
}

fn get_storage_path(key: &str) -> PathBuf {
    let base_dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("storage");
    let _ = std::fs::create_dir_all(&base_dir);
    base_dir.join(format!("{}.json", key))
}

pub struct DesktopEnv;

impl DesktopEnv {
    #[allow(unused)]
    async fn fetch_in_process<IN, OUT>(request: Request<IN>) -> Result<OUT, EnvError>
    where
        IN: Serialize + Send + 'static,
        for<'de> OUT: Deserialize<'de> + Send + 'static,
    {
        #[cfg(feature = "in-process")]
        {
            use tower::ServiceExt;

            // Acquire AppState from the server's GLOBAL_STATE
            let app_state = {
                let guard = stream_server::GLOBAL_STATE.read().map_err(|e| {
                    EnvError::Other(format!("Failed to read GLOBAL_STATE lock: {e}"))
                })?;
                guard.clone().ok_or_else(|| {
                    EnvError::Other("stream-server AppState is not initialized".to_string())
                })?
            };

            // Build the Axum router using the server's AppState
            let router = stream_server::build_router(app_state);

            // Construct the Tower request
            let (parts, body) = request.into_parts();
            let body_bytes =
                serde_json::to_vec(&body).map_err(|e| EnvError::Serde(e.to_string()))?;
            let axum_req = Request::from_parts(parts, axum::body::Body::from(body_bytes));

            // Call the router in-memory
            let response = router
                .oneshot(axum_req)
                .await
                .map_err(|e| EnvError::Fetch(e.to_string()))?;

            // Extract the body
            let body_data = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .map_err(|e| EnvError::Fetch(e.to_string()))?;

            let result: OUT =
                serde_json::from_slice(&body_data).map_err(|e| EnvError::Serde(e.to_string()))?;

            Ok(result)
        }
        #[cfg(not(feature = "in-process"))]
        {
            Err(EnvError::Other(
                "in-process feature is not enabled".to_string(),
            ))
        }
    }

    async fn fetch_http<IN, OUT>(request: Request<IN>) -> Result<OUT, EnvError>
    where
        IN: Serialize + Send + 'static,
        for<'de> OUT: Deserialize<'de> + Send + 'static,
    {
        let (parts, body) = request.into_parts();
        let client = get_http_client();
        let method = match parts.method {
            http::Method::GET => reqwest::Method::GET,
            http::Method::POST => reqwest::Method::POST,
            http::Method::PUT => reqwest::Method::PUT,
            http::Method::DELETE => reqwest::Method::DELETE,
            http::Method::HEAD => reqwest::Method::HEAD,
            _ => reqwest::Method::GET,
        };

        let url_str = parts.uri.to_string();
        tracing::debug!(method = ?parts.method, url = %url_str, "Sending Core API request");

        let mut req_builder = client.request(method, &url_str);

        for (key, val) in parts.headers.iter() {
            req_builder = req_builder.header(key.as_str(), val.as_bytes());
        }

        if parts.method != http::Method::GET {
            req_builder = req_builder.json(&body);
        }

        let start = std::time::Instant::now();
        let resp = req_builder.send().await.map_err(|e| {
            tracing::error!(url = %url_str, error = ?e, "Core API request failed");
            EnvError::Fetch(e.to_string())
        })?;

        let elapsed = start.elapsed().as_millis();
        if elapsed > 300 {
            tracing::warn!(
                url = %url_str,
                status = %resp.status(),
                elapsed_ms = elapsed,
                "Core API request took longer than threshold"
            );
        } else {
            tracing::debug!(
                url = %url_str,
                status = %resp.status(),
                elapsed_ms = elapsed,
                "Core API request completed"
            );
        }

        let val: OUT = resp
            .json()
            .await
            .map_err(|e| EnvError::Fetch(e.to_string()))?;

        Ok(val)
    }
}

impl Env for DesktopEnv {
    fn fetch<IN, OUT>(request: Request<IN>) -> TryEnvFuture<OUT>
    where
        IN: Serialize + Send + 'static,
        for<'de> OUT: Deserialize<'de> + Send + 'static,
    {
        let uri = request.uri().clone();
        let is_local = uri.host() == Some("127.0.0.1") || uri.host() == Some("localhost");

        if is_local && cfg!(feature = "in-process") {
            Self::fetch_in_process(request).boxed()
        } else {
            Self::fetch_http(request).boxed()
        }
    }

    fn get_storage<T: for<'de> Deserialize<'de> + Send + 'static>(
        key: &str,
    ) -> TryEnvFuture<Option<T>> {
        let _span = tracing::info_span!("get_storage", key = %key).entered();
        let path = get_storage_path(key);
        let result = if !path.exists() {
            tracing::info!(key = %key, "Storage file does not exist");
            Ok(None)
        } else {
            let start = std::time::Instant::now();
            std::fs::read_to_string(&path)
                .map_err(|e| EnvError::StorageReadError(e.to_string()))
                .and_then(|data| {
                    let parse_res =
                        serde_json::from_str(&data).map_err(|e| EnvError::Serde(e.to_string()));
                    tracing::info!(
                        key = %key,
                        bytes = data.len(),
                        elapsed_ms = start.elapsed().as_millis(),
                        "Loaded and parsed storage file"
                    );
                    parse_res
                })
                .map(Some)
        };
        future::ready(result).boxed()
    }

    fn set_storage<T: Serialize>(key: &str, value: Option<&T>) -> TryEnvFuture<()> {
        let _span = tracing::info_span!("set_storage", key = %key).entered();
        let path = get_storage_path(key);
        let result = match value {
            Some(v) => {
                let start = std::time::Instant::now();
                serde_json::to_string_pretty(v)
                    .map_err(|e| EnvError::Serde(e.to_string()))
                    .and_then(|data| {
                        let len = data.len();
                        let write_res = std::fs::write(&path, data)
                            .map_err(|e| EnvError::StorageWriteError(e.to_string()));
                        tracing::info!(
                            key = %key,
                            bytes = len,
                            elapsed_ms = start.elapsed().as_millis(),
                            "Serialized and saved storage file"
                        );
                        write_res
                    })
            }
            None => {
                if path.exists() {
                    let _ = std::fs::remove_file(&path);
                    tracing::info!(key = %key, "Removed storage file");
                }
                Ok(())
            }
        };
        future::ready(result).boxed()
    }

    fn exec_concurrent<F: Future<Output = ()> + Send + 'static>(future: F) {
        spawn_on_runtime(future);
    }

    fn exec_sequential<F: Future<Output = ()> + Send + 'static>(future: F) {
        spawn_on_runtime(future);
    }

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    fn flush_analytics() -> EnvFuture<'static, ()> {
        future::ready(()).boxed()
    }

    fn analytics_context(
        _ctx: &Ctx,
        _streaming_server: &StreamingServer,
        _path: &str,
    ) -> serde_json::Value {
        serde_json::Value::Null
    }

    #[cfg(debug_assertions)]
    fn log(message: String) {
        tracing::info!("{}", message);
    }
}
