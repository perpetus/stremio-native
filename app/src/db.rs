use std::path::PathBuf;
use std::sync::OnceLock;
use turso::{Builder, Connection, Database};

static DB: OnceLock<Database> = OnceLock::new();

#[tracing::instrument(skip(app_data_dir))]
pub async fn init_db(app_data_dir: PathBuf) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    // Ensure parent directories exist
    if !app_data_dir.exists() {
        std::fs::create_dir_all(&app_data_dir)?;
    }

    let db_path = app_data_dir.join("stremio.db");
    let db_path = db_path.to_string_lossy().into_owned();
    tracing::info!(path = %db_path, "Initializing Turso local database...");
    let db = Builder::new_local(&db_path).build().await?;

    let conn = db.connect()?;

    // Initialize tables: settings, logs, and image cache (storing BLOBs)
    conn.execute(
        "
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
    ",
        (),
    )
    .await?;

    conn.execute(
        "
        CREATE TABLE IF NOT EXISTS logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            level TEXT NOT NULL,
            message TEXT NOT NULL
        );
    ",
        (),
    )
    .await?;

    conn.execute(
        "
        CREATE TABLE IF NOT EXISTS image_cache (
            url TEXT PRIMARY KEY,
            image_data BLOB NOT NULL,
            downloaded_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
    ",
        (),
    )
    .await?;

    DB.set(db)
        .map_err(|_| anyhow::anyhow!("DB already initialized"))?;

    tracing::info!(
        elapsed_ms = start.elapsed().as_millis(),
        "Turso database schemas created/verified"
    );

    // Log database startup event
    let _ = insert_log("INFO", "Embedded Turso database initialized successfully.").await;

    Ok(())
}

pub fn get_conn() -> anyhow::Result<Connection> {
    DB.get()
        .ok_or_else(|| anyhow::anyhow!("DB not initialized"))?
        .connect()
        .map_err(Into::into)
}

// === Settings Helpers ===

#[tracing::instrument(skip(key))]
pub async fn get_setting(key: &str) -> anyhow::Result<Option<String>> {
    let start = std::time::Instant::now();
    let conn = get_conn()?;
    let mut rows = conn
        .query("SELECT value FROM settings WHERE key = ?", [key])
        .await?;
    let res = if let Some(row) = rows.next().await? {
        let val: String = row.get(0)?;
        Ok(Some(val))
    } else {
        Ok(None)
    };
    tracing::info!(
        key = %key,
        elapsed_ms = start.elapsed().as_millis(),
        success = res.is_ok(),
        found = res.as_ref().map(|opt| opt.is_some()).unwrap_or(false),
        "DB: get_setting"
    );
    res
}

#[tracing::instrument(skip(key, value))]
pub async fn set_setting(key: &str, value: &str) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let conn = get_conn()?;
    let res = conn
        .execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)",
            [key, value],
        )
        .await;
    tracing::info!(
        key = %key,
        elapsed_ms = start.elapsed().as_millis(),
        success = res.is_ok(),
        "DB: set_setting"
    );
    res.map(|_| ()).map_err(Into::into)
}

// === Logs Helpers ===

pub async fn insert_log(level: &str, message: &str) -> anyhow::Result<()> {
    let conn = get_conn()?;
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO logs (timestamp, level, message) VALUES (?, ?, ?)",
        (now, level.to_owned(), message.to_owned()),
    )
    .await?;
    Ok(())
}

pub async fn get_logs(limit: usize) -> anyhow::Result<Vec<String>> {
    let conn = get_conn()?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut rows = conn
        .query(
            "SELECT timestamp, level, message FROM logs ORDER BY id DESC LIMIT ?",
            [limit],
        )
        .await?;

    let mut entries = Vec::new();
    while let Some(row) = rows.next().await? {
        let ts: i64 = row.get(0)?;
        let level: String = row.get(1)?;
        let msg: String = row.get(2)?;

        let time_str = chrono::DateTime::from_timestamp(ts, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();

        entries.push(format!("[{}] [{}] {}", time_str, level, msg));
    }
    Ok(entries)
}

// === Image Cache Helpers ===

#[tracing::instrument(skip(url))]
pub async fn get_cached_image(url: &str) -> anyhow::Result<Option<Vec<u8>>> {
    let start = std::time::Instant::now();
    let conn = get_conn()?;
    let now = chrono::Utc::now().timestamp();
    let mut rows = conn
        .query(
            "SELECT image_data FROM image_cache WHERE url = ? AND expires_at > ?",
            (url.to_owned(), now),
        )
        .await?;

    let res = if let Some(row) = rows.next().await? {
        let bytes: Vec<u8> = row.get(0)?;
        Ok(Some(bytes))
    } else {
        Ok(None)
    };
    tracing::info!(
        url = %url,
        elapsed_ms = start.elapsed().as_millis(),
        found = res.as_ref().map(|opt| opt.is_some()).unwrap_or(false),
        "DB: get_cached_image"
    );
    res
}

#[tracing::instrument(skip(url, data, expiry_secs))]
pub async fn set_cached_image(url: &str, data: &[u8], expiry_secs: i64) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let conn = get_conn()?;
    let now = chrono::Utc::now().timestamp();
    let expires = now + expiry_secs;

    let res = conn.execute(
        "INSERT OR REPLACE INTO image_cache (url, image_data, downloaded_at, expires_at) VALUES (?, ?, ?, ?)",
        (url.to_owned(), data.to_vec(), now, expires),
    ).await;
    tracing::info!(
        url = %url,
        size_bytes = data.len(),
        elapsed_ms = start.elapsed().as_millis(),
        success = res.is_ok(),
        "DB: set_cached_image"
    );
    res.map(|_| ()).map_err(Into::into)
}
