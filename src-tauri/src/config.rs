//! Config loading + profile resolution.
//!
//! Config lives at the platform-conventional config dir under `opensplit/`:
//!   - Windows: `%APPDATA%\opensplit\config.toml`
//!   - Linux:   `~/.config/opensplit/config.toml`
//!   - macOS:   `~/Library/Application Support/opensplit/config.toml`
//!
//! On first launch we write a minimal config (no default_profile set), so the
//! launcher picker shows. Once the user picks something with "Set as default",
//! we persist it.

use std::{collections::HashMap, fs, path::PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Top-level config schema.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// Profile to auto-launch when `opensplit` runs with no arguments.
    ///
    /// When `None`, the frontend shows the launcher picker instead of spawning
    /// a pane immediately. The user can pick a tool (and optionally save it
    /// as the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,

    /// Inherit SSH session into newly-split panes when the source pane is in
    /// an `ssh` session.
    #[serde(default = "default_true")]
    pub ssh_inherit: bool,

    /// Reduce terminal repaint frequency for lower GPU usage.
    #[serde(default = "default_true")]
    pub low_gpu_mode: bool,

    /// User-defined launcher tile order. Unknown entries are ignored by the
    /// frontend; newly detected tools are appended after the saved order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub launcher_order: Vec<String>,

    /// Tools hidden from the launcher picker. They remain launchable from
    /// Settings, defaults, profiles, and the hidden-tools dropdown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_tools: Vec<String>,

    /// Named profiles. Empty by default; users add via Settings or by editing
    /// the file directly. Detection in `detect.rs` knows about common tools
    /// without requiring a profile entry.
    #[serde(default)]
    pub profiles: HashMap<String, Profile>,
}

fn default_true() -> bool {
    true
}

/// One launchable command preset.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Profile {
    /// Executable to run. Resolved against PATH if not absolute.
    pub command: String,
    /// Arguments passed to the command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional working directory; defaults to user's home.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Extra environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl Config {
    /// Minimal first-run config: no default, no profiles. The launcher picker
    /// + detection layer surface available tools without needing entries here.
    pub fn defaults() -> Self {
        Self {
            default_profile: None,
            ssh_inherit: true,
            low_gpu_mode: true,
            launcher_order: Vec::new(),
            hidden_tools: Vec::new(),
            profiles: HashMap::new(),
        }
    }

    /// Load from disk, creating a default file if none exists.
    pub fn load_or_create() -> Result<Self> {
        let path = config_path().context("could not determine config dir")?;
        if !path.exists() {
            let defaults = Self::defaults();
            save_to_disk(&path, &defaults)?;
            tracing::info!(path = %path.display(), "wrote default config");
            return Ok(defaults);
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let parsed: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(parsed)
    }

    /// Persist this config to disk, overwriting any existing file.
    pub fn save(&self) -> Result<PathBuf> {
        let path = config_path().context("could not determine config dir")?;
        save_to_disk(&path, self)?;
        Ok(path)
    }
}

fn save_to_disk(path: &PathBuf, cfg: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let toml = toml::to_string_pretty(cfg).context("failed to serialize config")?;
    let header = "# OpenSplit configuration.\n\
                  # Auto-generated; edit freely or use the in-app Settings panel.\n\
                  # https://github.com/flashosophy/OpenSplit\n\n";
    fs::write(path, format!("{header}{toml}"))
        .with_context(|| format!("writing config to {}", path.display()))?;
    Ok(())
}

/// Resolved spec ready to hand to the PTY layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: HashMap<String, String>,
    /// Profile name this was derived from, or `null` for raw commands.
    pub profile: Option<String>,
}

/// Look up a profile by name and convert to a `LaunchSpec`. Returns `None` if
/// no such profile exists.
pub fn profile_to_spec(config: &Config, name: &str) -> Option<LaunchSpec> {
    config.profiles.get(name).map(|p| LaunchSpec {
        command: p.command.clone(),
        args: p.args.clone(),
        cwd: p.cwd.clone(),
        env: p.env.clone(),
        profile: Some(name.to_string()),
    })
}

/// Construct a `LaunchSpec` from a detected tool (catalog entry). When the
/// user has a profile of the same name we prefer that (so their custom args
/// win); otherwise we synthesize one from the detection result.
pub fn spec_for_detected(config: &Config, name: &str, resolved_path: Option<&str>) -> LaunchSpec {
    if let Some(spec) = profile_to_spec(config, name) {
        return spec;
    }
    LaunchSpec {
        // Prefer the resolved absolute path when we have it (more reliable
        // than relying on PATH at spawn time, especially for GUI launches).
        command: resolved_path
            .map(|s| s.to_string())
            .unwrap_or_else(|| name.to_string()),
        args: vec![],
        cwd: None,
        env: HashMap::new(),
        profile: Some(name.to_string()),
    }
}

/// Returns the platform config file path (creating the parent dir is left to
/// callers that actually write to it).
pub fn config_path() -> Option<PathBuf> {
    let dir = dirs::config_dir()?;
    Some(dir.join("opensplit").join("config.toml"))
}
