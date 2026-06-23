//! OpenSplit Tauri backend library entry point.

use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;

mod config;
mod detect;
mod ipc;
mod pty;
mod session;

use ipc::{AppState, CliOverride};

const DEFAULT_WINDOW_WIDTH: f64 = 1280.0;
const DEFAULT_WINDOW_HEIGHT: f64 = 800.0;
const MIN_WINDOW_WIDTH: f64 = 480.0;
const MIN_WINDOW_HEIGHT: f64 = 320.0;
const LANDSCAPE_WINDOW_WIDTH_RATIO: f64 = 0.5;
const PORTRAIT_WINDOW_WIDTH_RATIO: f64 = 0.9;

/// CLI arguments understood by the `opensplit` binary.
#[derive(Debug, Parser)]
#[command(
    name = "opensplit",
    version,
    about = "Cross-platform terminal harness with right-click splits."
)]
struct Cli {
    /// Profile name (or bare command) to launch in the initial pane.
    /// When omitted and no `default_profile` is set, OpenSplit shows the
    /// launcher picker.
    profile: Option<String>,

    /// Run a raw command in the initial pane. Everything after `--` is the
    /// command + its args.
    #[arg(last = true)]
    raw: Vec<String>,
}

pub fn run() {
    let filter = EnvFilter::try_from_env("OPENSPLIT_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info,opensplit=debug"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();

    let cli = Cli::parse();

    let config = match config::Config::load_or_create() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("failed to load config, using defaults: {e:#}");
            config::Config::defaults()
        }
    };

    // Translate CLI into an optional override that wins over the config's
    // default_profile and over the picker.
    let cli_override = if !cli.raw.is_empty() {
        let raw = cli.raw;
        Some(CliOverride::Raw(config::LaunchSpec {
            command: raw[0].clone(),
            args: raw[1..].to_vec(),
            cwd: None,
            env: Default::default(),
            profile: None,
        }))
    } else {
        cli.profile.map(CliOverride::Profile)
    };

    tracing::info!(
        ?cli_override,
        default_profile = ?config.default_profile,
        "startup state"
    );

    let state = Arc::new(AppState::new(config, cli_override));

    // Window-state plugin: restore saved size but NOT position.
    // Position restoration is skipped because it can place the window
    // off-screen on multi-monitor setups when a secondary monitor is
    // disconnected between sessions.
    use tauri_plugin_window_state::StateFlags;
    let window_state = tauri_plugin_window_state::Builder::default()
        .with_state_flags(StateFlags::SIZE)
        .build();

    tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(window_state)
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            ipc::get_version,
            ipc::get_startup_action,
            ipc::get_shell_spec,
            ipc::detect_tools,
            ipc::get_tools_cached,
            ipc::list_profiles,
            ipc::get_config,
            ipc::set_default_profile,
            ipc::set_ssh_inherit,
            ipc::set_low_gpu_mode,
            ipc::set_launcher_order,
            ipc::set_tool_hidden,
            ipc::spawn_pane,
            ipc::write_pane,
            ipc::resize_pane,
            ipc::close_pane,
            ipc::pane_foreground_info,
            ipc::resolve_split_spec,
        ])
        .setup(|app| {
            use tauri::Manager;
            // On first ever launch (no saved window-state file), resize from
            // the active monitor. On subsequent launches the window-state
            // plugin restores the user's last size, so we leave it alone.
            let state_path = app
                .handle()
                .path()
                .app_data_dir()
                .map(|d| d.join("window-state.json"))
                .unwrap_or_default();

            if !state_path.exists() {
                if let Some(window) = app.handle().get_webview_window("main") {
                    if let Some(monitor) = window
                        .current_monitor()
                        .ok()
                        .flatten()
                        .or_else(|| window.primary_monitor().ok().flatten())
                    {
                        let size = monitor.size();
                        let scale = monitor.scale_factor();
                        let lw = size.width as f64 / scale;
                        let lh = size.height as f64 / scale;
                        let width_ratio = if lw < lh {
                            PORTRAIT_WINDOW_WIDTH_RATIO
                        } else {
                            LANDSCAPE_WINDOW_WIDTH_RATIO
                        };
                        let aspect = DEFAULT_WINDOW_HEIGHT / DEFAULT_WINDOW_WIDTH;
                        let mut target_w = (lw * width_ratio).round();
                        let mut target_h = (target_w * aspect).round();
                        let max_h = (lh * 0.9).round().max(1.0);

                        if target_h > max_h {
                            target_h = max_h;
                            target_w = (target_h / aspect).round();
                        }

                        target_w = target_w.max(MIN_WINDOW_WIDTH.min(lw)).min(lw.max(1.0));
                        target_h = target_h.max(MIN_WINDOW_HEIGHT.min(lh)).min(lh.max(1.0));
                        let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
                            width: target_w,
                            height: target_h,
                        }));
                        let cx = ((lw - target_w) / 2.0).max(0.0);
                        let cy = ((lh - target_h) / 2.0).max(0.0);
                        let _ =
                            window.set_position(tauri::Position::Logical(tauri::LogicalPosition {
                                x: cx,
                                y: cy,
                            }));
                    }
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running OpenSplit");
}
