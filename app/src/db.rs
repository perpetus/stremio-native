use std::sync::{
    OnceLock,
    atomic::{AtomicUsize, Ordering},
};
use std::{collections::HashMap, path::PathBuf};
use turso::{Builder, Connection, Database};

static DB: OnceLock<Database> = OnceLock::new();
static LOG_INSERTS_SINCE_CLEANUP: AtomicUsize = AtomicUsize::new(0);

const MAX_LOG_ROWS: i64 = 10_000;
const LOG_CLEANUP_INTERVAL: usize = 64;

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

    // Apply performance tuning PRAGMAs
    let _ = conn.execute("PRAGMA journal_mode = WAL;", ()).await;
    let _ = conn.execute("PRAGMA synchronous = NORMAL;", ()).await;
    let _ = conn.execute("PRAGMA temp_store = MEMORY;", ()).await;
    let _ = conn.execute("PRAGMA cache_size = -10000;", ()).await;

    // Initialize tables: settings, logs, and core_storage
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
        CREATE TABLE IF NOT EXISTS core_storage (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
    ",
        (),
    )
    .await?;

    // The active image pipeline uses the bounded memory cache and filesystem
    // cache. Remove the legacy BLOB table so existing installations can reuse
    // those database pages.
    conn.execute("DROP TABLE IF EXISTS image_cache", ()).await?;
    prune_logs(&conn).await?;

    // Migrate legacy JSON storage files to the SQLite database
    if let Err(e) = migrate_json_to_db(&conn, &app_data_dir).await {
        tracing::error!("Failed to run JSON database migration: {:?}", e);
    }

    let core_database = db.clone();
    DB.set(db)
        .map_err(|_| anyhow::anyhow!("DB already initialized"))?;
    if let Err(error) = core_env::install_database(core_database) {
        tracing::debug!(%error, "core storage database was initialized before app storage");
    }

    tracing::info!(
        elapsed_ms = start.elapsed().as_millis(),
        "Turso database schemas created/verified and optimizations applied"
    );

    // Log database startup event
    let _ = insert_log("INFO", "Embedded Turso database initialized successfully.").await;

    Ok(())
}

async fn migrate_json_to_db(
    conn: &Connection,
    app_data_dir: &std::path::Path,
) -> anyhow::Result<()> {
    // Check if profile.json exists to see if we need migration
    let profile_json_path = app_data_dir.join("profile.json");
    if !profile_json_path.exists() {
        return Ok(());
    }

    tracing::info!("Starting JSON files to Turso SQLite database migration...");

    let keys = [
        "profile",
        "library",
        "library_recent",
        "streams",
        "search_history",
        "server_urls",
        "notifications",
        "dismissed_events",
    ];

    for &key in &keys {
        let json_path = app_data_dir.join(format!("{}.json", key));
        if json_path.exists() {
            match std::fs::read_to_string(&json_path) {
                Ok(data) => {
                    let res = conn
                        .execute(
                            "INSERT OR REPLACE INTO core_storage (key, value) VALUES (?, ?)",
                            (key.to_owned(), data),
                        )
                        .await;
                    if let Err(e) = res {
                        tracing::error!("Migration: failed to save key {}: {:?}", key, e);
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "Migration: failed to read legacy file {}: {:?}",
                        json_path.display(),
                        e
                    );
                }
            }
        }
    }

    // Rename migrated files to .json.bak
    for &key in &keys {
        let json_path = app_data_dir.join(format!("{}.json", key));
        if json_path.exists() {
            let backup_path = app_data_dir.join(format!("{}.json.bak", key));
            if let Err(e) = std::fs::rename(&json_path, &backup_path) {
                tracing::warn!(
                    "Migration: failed to rename legacy file {}: {:?}",
                    json_path.display(),
                    e
                );
            }
        }
    }

    tracing::info!("JSON to SQLite migration completed successfully.");
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

#[tracing::instrument(skip(keys))]
pub async fn get_settings(keys: &[&str]) -> anyhow::Result<HashMap<String, String>> {
    if keys.is_empty() {
        return Ok(HashMap::new());
    }
    let conn = get_conn()?;
    let placeholders = std::iter::repeat_n("?", keys.len())
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!("SELECT key, value FROM settings WHERE key IN ({placeholders})");
    let mut rows = conn.query(&query, keys.to_vec()).await?;
    let mut settings = HashMap::with_capacity(keys.len());
    while let Some(row) = rows.next().await? {
        settings.insert(row.get(0)?, row.get(1)?);
    }
    Ok(settings)
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

#[tracing::instrument(skip(values))]
pub async fn set_settings(values: &[(&str, &str)]) -> anyhow::Result<()> {
    if values.is_empty() {
        return Ok(());
    }
    let mut conn = get_conn()?;
    let transaction = conn.transaction().await?;
    for &(key, value) in values {
        transaction
            .execute(
                "INSERT INTO settings (key, value) VALUES (?, ?) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value),
            )
            .await?;
    }
    transaction.commit().await?;
    Ok(())
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
    let previous_insert_count = LOG_INSERTS_SINCE_CLEANUP.fetch_add(1, Ordering::Relaxed);
    if previous_insert_count % LOG_CLEANUP_INTERVAL == LOG_CLEANUP_INTERVAL - 1 {
        prune_logs(&conn).await?;
    }
    Ok(())
}

async fn prune_logs(conn: &Connection) -> anyhow::Result<()> {
    conn.execute(
        "DELETE FROM logs
         WHERE id < COALESCE(
             (SELECT id FROM logs ORDER BY id DESC LIMIT 1 OFFSET ?),
             -1
         )",
        [MAX_LOG_ROWS - 1],
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
