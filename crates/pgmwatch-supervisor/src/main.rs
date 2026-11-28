#![cfg_attr(all(not(test), windows), windows_subsystem = "windows")]

mod chat_bridge;
mod clip_store;
mod dash_relay;
mod ghost_browser;
mod governor;
mod mse_stream;
mod resolver;
mod server;
mod stats;
mod supervisor;
mod tray_ui;
mod win32;
mod worker_bridge;

use pgmwatch_core::config::PgmWatchConfig;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use supervisor::SupervisorState;
use tracing_subscriber::fmt::MakeWriter;
use win32::{prevent_sleep_and_eco_qos, query_dxgi_adapters};

struct DualLogWriter {
    file: Option<Mutex<File>>,
    to_stdout: bool,
}

struct GuardWriter<'a> {
    writer: &'a DualLogWriter,
}

impl<'a> MakeWriter<'a> for DualLogWriter {
    type Writer = GuardWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        GuardWriter { writer: self }
    }
}

impl Write for GuardWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.writer.to_stdout {
            let _ = std::io::stdout().write_all(buf);
        }
        if let Some(ref f_mtx) = self.writer.file
            && let Ok(mut f) = f_mtx.lock()
        {
            let _ = f.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.writer.to_stdout {
            let _ = std::io::stdout().flush();
        }
        if let Some(ref f_mtx) = self.writer.file
            && let Ok(mut f) = f_mtx.lock()
        {
            let _ = f.flush();
        }
        Ok(())
    }
}

fn init_logging(log_path: &Path, to_stdout: bool) {
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Truncate log on startup if larger than 5 MB so portable folders stay compact.
    if let Ok(meta) = std::fs::metadata(log_path)
        && meta.len() > 5 * 1024 * 1024
    {
        let _ = std::fs::write(log_path, b"");
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .ok()
        .map(Mutex::new);

    let writer = DualLogWriter { file, to_stdout };
    let _ = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "pgmwatch_supervisor=info,info".to_string()),
        )
        .try_init();
}

#[tokio::main]
async fn main() {
    let headless = std::env::args().any(|a| a == "--headless")
        || std::env::var_os("PGMWATCH_HEADLESS").is_some();

    #[cfg(windows)]
    if headless {
        unsafe {
            let _ = windows::Win32::System::Console::AttachConsole(
                windows::Win32::System::Console::ATTACH_PARENT_PROCESS,
            );
        }
    }

    let base_dir = detect_base_dir();
    let cfg = match PgmWatchConfig::load_or_create(&base_dir.join("pgmwatch.toml")) {
        Ok(c) => c,
        Err(err) => {
            show_startup_error(&format!("Failed to load pgmwatch.toml:\n{err}"), headless);
            std::process::exit(1);
        }
    };

    let data_dir = cfg.resolve_data_dir(&base_dir);
    let log_path = data_dir.join("pgmwatch.log");
    init_logging(&log_path, headless);

    prevent_sleep_and_eco_qos();

    if let Err(err) = run_app(cfg, base_dir, log_path, headless).await {
        tracing::error!("PGMWatch fatal error: {err:#}");
        show_startup_error(&format!("PGMWatch failed to start:\n{err}"), headless);
        std::process::exit(1);
    }
}

async fn run_app(
    cfg: PgmWatchConfig,
    base_dir: PathBuf,
    log_path: PathBuf,
    headless: bool,
) -> anyhow::Result<()> {
    let adapters = query_dxgi_adapters();
    if adapters.is_empty() {
        tracing::info!("DXGI self-test: running in CPU/fallback mode");
    } else {
        for a in &adapters {
            tracing::info!(
                "DXGI adapter detected: {} (LUID {}, budget {} MB)",
                a.name,
                a.luid_hex,
                a.vram_budget_mb
            );
        }
    }

    let addr: SocketAddr = format!("{}:{}", cfg.server.bind_host, cfg.server.port).parse()?;
    let url = format!("http://{addr}");

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(err) => {
            anyhow::bail!(
                "Could not bind {url} ({err}).\nPGMWatch may already be running or another program is using port {}.",
                cfg.server.port
            );
        }
    };

    let state = SupervisorState::init(cfg.clone(), base_dir).await?;
    state.start_background_tasks().await;

    let app = server::build_router(state.clone());
    tracing::info!("PGMWatch supervisor listening at {url}");

    if !headless {
        tray_ui::run_launcher_window(
            state.clone(),
            tray_ui::LauncherConfig {
                url: url.clone(),
                log_path,
            },
        );
    }

    if cfg.server.open_browser_on_start && std::env::var_os("PGMWATCH_NO_BROWSER").is_none() {
        open_browser(&url);
    }

    let mut drain_watch = state.subscribe_shutdown();
    let drain_timeout = async move {
        while !*drain_watch.borrow() {
            if drain_watch.changed().await.is_err() {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        tracing::warn!("HTTP connection drain timed out after 2s; forcing shutdown");
    };
    tokio::select! {
        res = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal(state.clone())) => {
            if let Err(err) = res {
                tracing::error!("HTTP server exited unexpectedly: {err}");
            }
        }
        _ = drain_timeout => {}
    }
    state.request_shutdown();
    state.mse_hubs.invalidate_all().await;
    tracing::info!("Stopping active channel workers...");
    state.stop_all_workers().await;
    state.db.checkpoint_wal_now().await;
    tracing::info!("PGMWatch shutdown complete");
    Ok(())
}

#[cfg(windows)]
pub(crate) fn open_browser(url: &str) {
    let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
}

#[cfg(not(windows))]
pub(crate) fn open_browser(url: &str) {
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

#[cfg(windows)]
fn show_startup_error(msg: &str, headless: bool) {
    if headless {
        eprintln!("{msg}");
        return;
    }
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let text: Vec<u16> = OsStr::new(msg).encode_wide().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = OsStr::new("PGMWatch").encode_wide().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONERROR);
    }
}

#[cfg(not(windows))]
fn show_startup_error(msg: &str, _headless: bool) {
    eprintln!("{msg}");
}

async fn shutdown_signal(state: Arc<SupervisorState>) {
    let mut rx = state.subscribe_shutdown();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Ctrl+C received; initiating clean shutdown");
            state.request_shutdown();
        }
        _ = async {
            while !*rx.borrow() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        } => {
            tracing::info!("Shutdown trigger received; stopping HTTP server");
        }
    }
}

fn detect_base_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
        && (dir.join("pgmwatch.toml").exists()
            || (dir.join("pgmwatch-worker.exe").exists() && dir.join("bin").exists()))
    {
        return dir.to_path_buf();
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
