use std::io::Write;
use std::path::PathBuf;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::prelude::*;

use crate::performance::ProfileConfig;

fn should_record(metadata: &tracing::Metadata<'_>) -> bool {
    *metadata.level() <= tracing::Level::INFO
        && !(metadata.is_span() && metadata.name() == "request")
}

pub struct LoggerGuards {
    pub _file_guard: tracing_appender::non_blocking::WorkerGuard,
    #[cfg(debug_assertions)]
    pub _chrome_guard: Option<tracing_chrome::FlushGuard>,
}

pub fn init_logger(profile: &ProfileConfig) -> anyhow::Result<LoggerGuards> {
    let log_dir = PathBuf::from("storage").join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("stremio.log");
    // A fixed, truncated file makes each report correspond to exactly one run.
    let log_file = std::fs::File::create(&log_path)?;
    let (file_writer, file_guard) = tracing_appender::non_blocking(log_file);

    #[cfg(debug_assertions)]
    let (chrome_layer, chrome_guard) = if profile.mode.enabled() {
        let mut builder = tracing_chrome::ChromeLayerBuilder::new()
            .include_args(true)
            .include_locations(false);
        if let Some(output) = profile.output.as_ref() {
            if let Some(parent) = output
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent)?;
            }
            builder = builder.writer(std::fs::File::create(output)?);
        }
        let (layer, guard) = builder.build();
        let mode = profile.mode;
        let layer = layer.with_filter(tracing_subscriber::filter::filter_fn(move |metadata| {
            mode.includes_target(metadata.target()) && should_record(metadata)
        }));
        (Some(layer), Some(guard))
    } else {
        (None, None)
    };

    #[cfg(debug_assertions)]
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::io::stderr.and(file_writer))
                .with_filter(tracing_subscriber::filter::filter_fn(should_record)),
        )
        .with(chrome_layer)
        .init();

    #[cfg(not(debug_assertions))]
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::io::stderr.and(file_writer))
                .with_filter(tracing_subscriber::filter::filter_fn(should_record)),
        )
        .init();

    // Also append panics synchronously. This preserves the failure even when a
    // release build aborts before the non-blocking writer can drain its queue.
    let panic_log_path = log_path.clone();
    let default_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(panic = %panic_info, %backtrace, "uncaught panic");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&panic_log_path)
        {
            let _ = writeln!(file, "PANIC: {panic_info}");
            let _ = writeln!(file, "BACKTRACE:\n{backtrace}");
            let _ = file.flush();
        }
        default_panic_hook(panic_info);
    }));

    tracing::info!(path = %log_path.display(), "file logging initialized");
    if profile.mode.enabled() {
        tracing::info!(mode = ?profile.mode, output = ?profile.output, "performance profiling enabled");
        tracing::info!(
            "Trace file will be saved in current directory on exit. Drag and drop it into https://ui.perfetto.dev to view visual timeline."
        );
    }

    Ok(LoggerGuards {
        _file_guard: file_guard,
        #[cfg(debug_assertions)]
        _chrome_guard: chrome_guard,
    })
}
