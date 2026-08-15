//! Tauri command handlers + shared `AppState`.
//!
//! Frontend talks to backend exclusively through these `#[tauri::command]`
//! functions. PTY output flows the other way as `pane:data` events emitted
//! from the reader thread (see `pty.rs`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use crate::{
    config::{self, Config, LaunchSpec},
    detect::{self, DetectedTool},
    pty::{self, PaneRegistry},
    session,
};

/// Process-wide state managed by Tauri.
pub struct AppState {
    pub config: parking_lot::RwLock<Config>,
    /// CLI-provided override: `Some(name)` from positional arg, or raw command
    /// from `-- cmd ...`. When set, the launcher picker is skipped.
    pub cli_override: parking_lot::RwLock<Option<CliOverride>>,
    pub panes: PaneRegistry,
    /// Cached detection results. Built on first call, invalidated when the
    /// user presses "Refresh" in Settings. Fast enough to re-scan on demand
    /// but caching means right-click is instantaneous.
    pub cached_tools: parking_lot::Mutex<Option<Vec<DetectedTool>>>,
}

#[derive(Debug, Clone)]
pub enum CliOverride {
    /// User typed `opensplit <name>`.
    Profile(String),
    /// User typed `opensplit -- cmd args...`.
    Raw(LaunchSpec),
}

impl AppState {
    pub fn new(config: Config, cli_override: Option<CliOverride>) -> Self {
        Self {
            config: parking_lot::RwLock::new(config),
            cli_override: parking_lot::RwLock::new(cli_override),
            panes: PaneRegistry::default(),
            cached_tools: parking_lot::Mutex::new(None),
        }
    }
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ---------------------------------------------------------------------------
// Version
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct VersionInfo {
    pub semver: &'static str,
    pub git_hash: &'static str,
    pub build_date: &'static str,
    /// Human-friendly display string, e.g. "0.1.0 · 2026-05-15 · abc12345"
    pub display: String,
}

#[tauri::command]
pub fn get_version() -> VersionInfo {
    let semver = env!("CARGO_PKG_VERSION");
    let hash = env!("OPENSPLIT_GIT_HASH");
    let date = env!("OPENSPLIT_BUILD_DATE");
    VersionInfo {
        semver,
        git_hash: hash,
        build_date: date,
        display: format!("{semver} · {date} · {hash}"),
    }
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

/// What the frontend should do on app start.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StartupAction {
    /// Skip the picker and immediately spawn this spec.
    Launch { spec: LaunchSpec },
    /// Show the picker.
    Picker {
        detected: Vec<DetectedTool>,
        /// True if no AI-category tools were detected. UI may use this to
        /// emphasize the shell button or fall back to it after a timeout.
        no_ai_tools: bool,
    },
}

/// Runs on the async runtime (not the UI thread): may scan PATH via
/// `detect_all`, which is filesystem-bound and can take hundreds of ms.
#[tauri::command]
pub async fn get_startup_action(app: AppHandle) -> StartupAction {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let cfg = state.config.read();

    // 1. CLI override always wins.
    if let Some(over) = state.cli_override.read().clone() {
        match over {
            CliOverride::Raw(spec) => return StartupAction::Launch { spec },
            CliOverride::Profile(name) => {
                let spec = config::profile_to_spec(&cfg, &name).unwrap_or_else(|| {
                    // Synthesize: treat unknown name as bare command.
                    LaunchSpec {
                        command: name.clone(),
                        args: vec![],
                        cwd: None,
                        env: Default::default(),
                        profile: None,
                    }
                });
                return StartupAction::Launch { spec };
            }
        }
    }

    // 2. Configured default profile, if it exists.
    if let Some(name) = &cfg.default_profile {
        // Try profile first, then synthesize from detection so users can set
        // a default like "claude" without manually adding a profile entry.
        if let Some(spec) = config::profile_to_spec(&cfg, name) {
            return StartupAction::Launch { spec };
        }
        let detected = detect::detect_all(&cfg.profiles);
        if let Some(tool) = detected.iter().find(|t| &t.name == name) {
            let spec = config::spec_for_detected(&cfg, name, tool.path.as_deref());
            return StartupAction::Launch { spec };
        }
        tracing::warn!(
            "default_profile `{name}` not found in profiles or detection; showing picker"
        );
    }

    // 3. No default → show picker. If literally no AI tools, picker still
    //    shows but UI may opt to auto-fall-through to shell.
    let detected = detect::detect_all(&cfg.profiles);
    let no_ai = !detect::any_ai_tool_detected(&detected);
    StartupAction::Picker {
        detected,
        no_ai_tools: no_ai,
    }
}

/// Returns a `LaunchSpec` for the system's default shell (the "fallback" that
/// a pane respawns into after the foreground app exits).
///
/// Prefers the user's configured `shell` profile if present; otherwise
/// synthesizes from the platform default (pwsh → powershell → cmd on Windows;
/// $SHELL → bash → sh on Unix). The returned spec always sets `cwd` to the
/// caller-supplied directory so the shell opens in the right place.
/// Runs on the async runtime: falls back to a PATH scan (`detect_all`) when
/// no "shell" profile is configured, which is the default.
#[tauri::command]
pub async fn get_shell_spec(app: AppHandle, cwd: Option<String>) -> LaunchSpec {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let cfg = state.config.read();
    // Prefer the user's explicit "shell" profile.
    if let Some(mut spec) = config::profile_to_spec(&cfg, "shell") {
        spec.cwd = cwd;
        return spec;
    }
    // Fall back to detection (detect_all always returns a "shell" entry).
    let detected = detect::detect_all(&cfg.profiles);
    if let Some(tool) = detected.iter().find(|t| t.name == "shell") {
        let mut spec = config::spec_for_detected(&cfg, "shell", tool.path.as_deref());
        spec.cwd = cwd;
        return spec;
    }
    // Last resort — should be unreachable in practice.
    LaunchSpec {
        command: default_shell_command(),
        args: default_shell_args(),
        cwd,
        env: Default::default(),
        profile: Some("shell".to_string()),
    }
}

#[cfg(windows)]
fn default_shell_command() -> String {
    "cmd.exe".into()
}
#[cfg(windows)]
fn default_shell_args() -> Vec<String> {
    vec![]
}
#[cfg(not(windows))]
fn default_shell_command() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}
#[cfg(not(windows))]
fn default_shell_args() -> Vec<String> {
    vec!["-l".into()]
}

// ---------------------------------------------------------------------------
// Detection + profiles
// ---------------------------------------------------------------------------

/// Re-scan PATH for installed tools. Invalidates + rebuilds the cache.
/// Called by the Settings panel Refresh button and the initial boot.
/// Runs on the async runtime: full PATH×PATHEXT filesystem scan.
#[tauri::command]
pub async fn detect_tools(app: AppHandle) -> Vec<DetectedTool> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let cfg = state.config.read();
    let tools = detect::detect_all(&cfg.profiles);
    *state.cached_tools.lock() = Some(tools.clone());
    tools
}

/// Return cached detection results without re-scanning. If no cache exists yet
/// (e.g. right-click before Settings was opened), scans once and caches.
/// Used by the context menu switch submenu so right-clicking is instant.
/// Runs on the async runtime: the cold path performs a PATH scan.
#[tauri::command]
pub async fn get_tools_cached(app: AppHandle) -> Vec<DetectedTool> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let mut cache = state.cached_tools.lock();
    if let Some(ref tools) = *cache {
        return tools.clone();
    }
    let cfg = state.config.read();
    let tools = detect::detect_all(&cfg.profiles);
    *cache = Some(tools.clone());
    tools
}

#[derive(Debug, Serialize)]
pub struct ProfileSummary {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

#[tauri::command]
pub fn list_profiles(state: State<'_, Arc<AppState>>) -> Vec<ProfileSummary> {
    let cfg = state.config.read();
    let mut out: Vec<ProfileSummary> = cfg
        .profiles
        .iter()
        .map(|(name, p)| ProfileSummary {
            name: name.clone(),
            command: p.command.clone(),
            args: p.args.clone(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[derive(Debug, Serialize)]
pub struct ConfigSnapshot {
    pub default_profile: Option<String>,
    pub ssh_inherit: bool,
    /// Terminal repaint throttle interval in milliseconds. `0` disables it.
    pub low_gpu_update_ms: u32,
    pub launcher_order: Vec<String>,
    pub hidden_tools: Vec<String>,
    pub config_path: Option<String>,
}

#[tauri::command]
pub fn get_config(state: State<'_, Arc<AppState>>) -> ConfigSnapshot {
    let cfg = state.config.read();
    ConfigSnapshot {
        default_profile: cfg.default_profile.clone(),
        ssh_inherit: cfg.ssh_inherit,
        low_gpu_update_ms: cfg.low_gpu_update_ms,
        launcher_order: cfg.launcher_order.clone(),
        hidden_tools: cfg.hidden_tools.clone(),
        config_path: config::config_path().map(|p| p.display().to_string()),
    }
}

#[derive(Debug, Deserialize)]
pub struct SetDefaultProfileArgs {
    /// `None` clears the default (so picker shows on next launch).
    pub name: Option<String>,
}

#[tauri::command]
pub fn set_default_profile(
    state: State<'_, Arc<AppState>>,
    args: SetDefaultProfileArgs,
) -> Result<ConfigSnapshot, String> {
    {
        let mut cfg = state.config.write();
        cfg.default_profile = args.name;
        cfg.save().map_err(err)?;
    }
    Ok(get_config(state))
}

#[derive(Debug, Deserialize)]
pub struct SetSshInheritArgs {
    pub enabled: bool,
}

#[tauri::command]
pub fn set_ssh_inherit(
    state: State<'_, Arc<AppState>>,
    args: SetSshInheritArgs,
) -> Result<ConfigSnapshot, String> {
    {
        let mut cfg = state.config.write();
        cfg.ssh_inherit = args.enabled;
        cfg.save().map_err(err)?;
    }
    Ok(get_config(state))
}

#[derive(Debug, Deserialize)]
pub struct SetLowGpuUpdateMsArgs {
    /// New throttle interval in milliseconds. `0` disables throttling.
    pub low_gpu_update_ms: u32,
}

#[tauri::command]
pub fn set_low_gpu_update_ms(
    state: State<'_, Arc<AppState>>,
    args: SetLowGpuUpdateMsArgs,
) -> Result<ConfigSnapshot, String> {
    {
        let mut cfg = state.config.write();
        cfg.low_gpu_update_ms = args.low_gpu_update_ms;
        cfg.save().map_err(err)?;
    }
    Ok(get_config(state))
}

fn unique_tool_names(names: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for name in names {
        let name = name.trim();
        if !name.is_empty() && !out.iter().any(|existing: &String| existing == name) {
            out.push(name.to_string());
        }
    }
    out
}

#[derive(Debug, Deserialize)]
pub struct SetLauncherOrderArgs {
    pub order: Vec<String>,
}

#[tauri::command]
pub fn set_launcher_order(
    state: State<'_, Arc<AppState>>,
    args: SetLauncherOrderArgs,
) -> Result<ConfigSnapshot, String> {
    {
        let mut cfg = state.config.write();
        cfg.launcher_order = unique_tool_names(args.order);
        cfg.save().map_err(err)?;
    }
    Ok(get_config(state))
}

#[derive(Debug, Deserialize)]
pub struct SetToolHiddenArgs {
    pub name: String,
    pub hidden: bool,
}

#[tauri::command]
pub fn set_tool_hidden(
    state: State<'_, Arc<AppState>>,
    args: SetToolHiddenArgs,
) -> Result<ConfigSnapshot, String> {
    {
        let mut cfg = state.config.write();
        let mut hidden = unique_tool_names(cfg.hidden_tools.clone());
        let name = args.name.trim();
        if !name.is_empty() {
            if args.hidden {
                if !hidden.iter().any(|existing| existing == name) {
                    hidden.push(name.to_string());
                }
            } else {
                hidden.retain(|existing| existing != name);
            }
        }
        cfg.hidden_tools = hidden;
        cfg.save().map_err(err)?;
    }
    Ok(get_config(state))
}

// ---------------------------------------------------------------------------
// PTY lifecycle
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SpawnPaneArgs {
    /// One of:
    /// - `{ kind: "detected", name }` → look up by detection name
    /// - `{ kind: "profile",  name }` → look up profile by name
    /// - `{ kind: "spec",     spec }` → use a pre-resolved spec (used for SSH inheritance)
    pub source: SpawnSource,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SpawnSource {
    Detected { name: String },
    Profile { name: String },
    Spec { spec: LaunchSpec },
}

#[derive(Debug, Serialize)]
pub struct SpawnPaneResult {
    pub pane_id: String,
    pub spec: LaunchSpec,
}

/// Runs on the async runtime: program resolution touches the filesystem
/// (PATH + PATHEXT probing) and spawning a ConPTY can block; doing this on
/// the UI thread froze the whole window (keystrokes, output events, and all
/// other invokes queue behind it).
#[tauri::command]
pub async fn spawn_pane(
    app: AppHandle,
    args: SpawnPaneArgs,
) -> Result<SpawnPaneResult, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let spec = {
        let cfg = state.config.read();
        match args.source {
            SpawnSource::Detected { name } => {
                let detected = detect::detect_all(&cfg.profiles);
                let tool = detected
                    .iter()
                    .find(|t| t.name == name)
                    .ok_or_else(|| format!("detected tool `{name}` not found"))?;
                config::spec_for_detected(&cfg, &name, tool.path.as_deref())
            }
            SpawnSource::Profile { name } => config::profile_to_spec(&cfg, &name)
                .ok_or_else(|| format!("no such profile: {name}"))?,
            SpawnSource::Spec { spec } => spec,
        }
    };
    let pane = pty::spawn(&app, spec.clone(), args.cols.max(1), args.rows.max(1)).map_err(err)?;
    let id = pane.id.clone();
    state.panes.insert(pane);
    Ok(SpawnPaneResult { pane_id: id, spec })
}

#[derive(Debug, Deserialize)]
pub struct WritePaneArgs {
    pub pane_id: String,
    /// Raw bytes from xterm.js. Treated as UTF-8.
    pub data: String,
}

#[tauri::command]
pub fn write_pane(state: State<'_, Arc<AppState>>, args: WritePaneArgs) -> Result<(), String> {
    let pane = pty::require(&state.panes, &args.pane_id).map_err(err)?;
    pane.write(args.data.as_bytes()).map_err(err)
}

#[derive(Debug, Deserialize)]
pub struct ResizePaneArgs {
    pub pane_id: String,
    pub cols: u16,
    pub rows: u16,
}

#[tauri::command]
pub fn resize_pane(state: State<'_, Arc<AppState>>, args: ResizePaneArgs) -> Result<(), String> {
    let pane = pty::require(&state.panes, &args.pane_id).map_err(err)?;
    pane.resize(args.cols.max(1), args.rows.max(1)).map_err(err)
}

#[derive(Debug, Deserialize)]
pub struct ClosePaneArgs {
    pub pane_id: String,
}

#[tauri::command]
pub fn close_pane(state: State<'_, Arc<AppState>>, args: ClosePaneArgs) -> Result<(), String> {
    if let Some(pane) = state.panes.remove(&args.pane_id) {
        let _ = pane.kill();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// SSH inheritance helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PaneForegroundArgs {
    pub pane_id: String,
}

/// Runs on the async runtime: `session::foreground` refreshes the entire
/// process table (per-process cmd/cwd reads). It is called on every
/// right-click (SSH detection) — on the UI thread a slow sweep froze the
/// whole app.
#[tauri::command]
pub async fn pane_foreground_info(
    app: AppHandle,
    args: PaneForegroundArgs,
) -> Result<Option<session::ForegroundInfo>, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let pane = pty::require(&state.panes, &args.pane_id).map_err(err)?;
    let pid = match pane.child_pid() {
        Some(p) => p,
        None => return Ok(None),
    };
    Ok(session::foreground(pid))
}

#[derive(Debug, Deserialize)]
pub struct ResolveSplitSpecArgs {
    pub source_pane_id: String,
    /// Optional profile to fall back to if SSH isn't detected (or inherit is off).
    pub fallback_profile: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResolveSplitSpecResult {
    pub spec: LaunchSpec,
    pub inherited_ssh: bool,
    pub source_foreground: Option<session::ForegroundInfo>,
}

/// Runs on the async runtime: performs the full process-table sweep that
/// `session::foreground` needs (see `pane_foreground_info`).
#[tauri::command]
pub async fn resolve_split_spec(
    app: AppHandle,
    args: ResolveSplitSpecArgs,
) -> Result<ResolveSplitSpecResult, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    // Resolve everything we need from the config, then DROP the read lock
    // before the (slow) process scan — otherwise every `set_*` command
    // blocks for the whole sweep.
    let (inherit_enabled, fallback) = {
        let cfg = state.config.read();
        let inherit_enabled = cfg.ssh_inherit;

        let fallback = if let Some(name) = args.fallback_profile.as_deref() {
            // Try profile, then detection, then source pane's own spec.
            if let Some(s) = config::profile_to_spec(&cfg, name) {
                s
            } else {
                let detected = detect::detect_all(&cfg.profiles);
                if let Some(tool) = detected.iter().find(|t| t.name == name) {
                    config::spec_for_detected(&cfg, name, tool.path.as_deref())
                } else {
                    state
                        .panes
                        .get(&args.source_pane_id)
                        .map(|p| p.spec().clone())
                        .unwrap_or_else(|| LaunchSpec {
                            command: name.to_string(),
                            args: vec![],
                            cwd: None,
                            env: Default::default(),
                            profile: None,
                        })
                }
            }
        } else {
            state
                .panes
                .get(&args.source_pane_id)
                .map(|p| p.spec().clone())
                .ok_or_else(|| format!("unknown source pane {}", args.source_pane_id))?
        };

        (inherit_enabled, fallback)
    };

    if !inherit_enabled {
        return Ok(ResolveSplitSpecResult {
            spec: fallback,
            inherited_ssh: false,
            source_foreground: None,
        });
    }

    let fg = state
        .panes
        .get(&args.source_pane_id)
        .and_then(|p| p.child_pid())
        .and_then(session::foreground);

    let Some(fg) = fg else {
        return Ok(ResolveSplitSpecResult {
            spec: fallback,
            inherited_ssh: false,
            source_foreground: None,
        });
    };

    let inherited = fg.is_ssh;
    let spec = session::build_split_spec(&fg, fallback);
    Ok(ResolveSplitSpecResult {
        spec,
        inherited_ssh: inherited,
        source_foreground: Some(fg),
    })
}
