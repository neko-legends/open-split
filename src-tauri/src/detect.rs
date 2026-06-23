//! Detection of installed CLI tools.
//!
//! On launch (and on-demand from Settings), we scan PATH for a curated list of
//! known AI coding assistants and shells. Results are merged with the user's
//! configured profiles so the launcher picker shows everything available.

use std::path::PathBuf;

use serde::Serialize;

/// One detection candidate in the catalog.
struct Candidate {
    /// Stable profile-style name used as the candidate's identifier and as
    /// the default profile name if the user picks "Set as default".
    name: &'static str,
    /// Human-friendly display label (button text).
    label: &'static str,
    /// Short one-line description shown under the button.
    description: &'static str,
    /// Single-letter category icon hint for the frontend (e.g. "A" for AI, "T" for terminal).
    /// Frontend may render an actual SVG based on this.
    icon: &'static str,
    /// Bare command name we'll search for (no extension; PATHEXT handles that on Windows).
    /// We try each of these and use the first one found.
    binaries: &'static [&'static str],
}

/// The static catalog. Order here is the order shown in the picker.
const CATALOG: &[Candidate] = &[
    Candidate {
        name: "shell",
        label: "Default Terminal",
        description: "System shell (PowerShell / bash / zsh)",
        icon: "terminal",
        binaries: &[], // resolved specially; see default_shell_*
    },
    Candidate {
        name: "opencode",
        label: "opencode",
        description: "Open-source AI coding agent",
        icon: "ai",
        binaries: &["opencode"],
    },
    Candidate {
        name: "codex",
        label: "Codex",
        description: "OpenAI Codex CLI",
        icon: "ai",
        binaries: &["codex"],
    },
    Candidate {
        name: "claude",
        label: "Claude Code",
        description: "Anthropic's coding agent",
        icon: "ai",
        binaries: &["claude"],
    },
    Candidate {
        name: "gemini",
        label: "Gemini CLI",
        description: "Google Gemini coding agent",
        icon: "ai",
        binaries: &["gemini"],
    },
    Candidate {
        name: "grok",
        label: "Grok Build",
        description: "xAI coding agent and CLI",
        icon: "ai",
        binaries: &["grok-build", "grok"],
    },
    Candidate {
        name: "hermes",
        label: "Hermes",
        description: "Nous Research Hermes Agent",
        icon: "ai",
        binaries: &["hermes"],
    },
    Candidate {
        name: "aider",
        label: "aider",
        description: "AI pair programming in your terminal",
        icon: "ai",
        binaries: &["aider"],
    },
    Candidate {
        name: "cursor-agent",
        label: "Cursor Agent",
        description: "Cursor's CLI agent",
        icon: "ai",
        binaries: &["cursor-agent"],
    },
    Candidate {
        name: "kimi",
        label: "Kimi Code",
        description: "Moonshot AI coding agent (Kimi K2)",
        icon: "ai",
        // Installed via: curl -L code.kimi.com/install.sh | bash
        binaries: &["kimi-cli"],
    },
    Candidate {
        name: "sgpt",
        label: "ShellGPT",
        description: "ChatGPT in your shell",
        icon: "ai",
        binaries: &["sgpt"],
    },
];

/// One detected tool returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct DetectedTool {
    /// Profile-style identifier (e.g. "opencode", "claude", "shell").
    pub name: String,
    /// Display label for the button.
    pub label: String,
    pub description: String,
    /// Icon kind hint: "ai" | "terminal" | "custom".
    pub icon: String,
    /// Absolute resolved path on disk, or `None` when the tool was found
    /// via the special "shell" path.
    pub path: Option<String>,
    /// True if this came from the static catalog; false if it's a user-defined
    /// profile that doesn't map to anything we know about.
    pub builtin: bool,
    /// True if a corresponding profile already exists in user config.
    /// Frontend can use this to dim/badge entries that are "already saved".
    pub has_profile: bool,
}

/// Scan PATH for everything in the catalog plus the user's named profiles.
///
/// Each catalog entry is reported only if at least one of its binaries
/// resolves; the "shell" entry is always reported and points at the platform
/// default shell.
///
/// User profiles whose command doesn't appear in the catalog are appended at
/// the end so user-defined launchers show up in the picker too.
pub fn detect_all(
    profiles: &std::collections::HashMap<String, crate::config::Profile>,
) -> Vec<DetectedTool> {
    let mut out: Vec<DetectedTool> = Vec::new();

    for cand in CATALOG {
        if cand.name == "shell" {
            // The shell entry is always available.
            let shell_path = default_shell_path();
            out.push(DetectedTool {
                name: cand.name.to_string(),
                label: cand.label.to_string(),
                description: cand.description.to_string(),
                icon: cand.icon.to_string(),
                path: shell_path.map(|p| p.display().to_string()),
                builtin: true,
                has_profile: profiles.contains_key(cand.name),
            });
            continue;
        }

        let resolved = cand.binaries.iter().find_map(|b| which(b));
        if let Some(path) = resolved {
            out.push(DetectedTool {
                name: cand.name.to_string(),
                label: cand.label.to_string(),
                description: cand.description.to_string(),
                icon: cand.icon.to_string(),
                path: Some(path.display().to_string()),
                builtin: true,
                has_profile: profiles.contains_key(cand.name),
            });
        }
    }

    // Append user profiles that don't correspond to anything in the catalog.
    let known: std::collections::HashSet<&str> = CATALOG.iter().map(|c| c.name).collect();
    let mut user_extras: Vec<(&String, &crate::config::Profile)> = profiles
        .iter()
        .filter(|(name, _)| !known.contains(name.as_str()))
        .collect();
    user_extras.sort_by(|a, b| a.0.cmp(b.0));

    for (name, profile) in user_extras {
        let resolved_path = which(&profile.command).map(|p| p.display().to_string());
        out.push(DetectedTool {
            name: name.clone(),
            label: name.clone(),
            description: format!("Custom profile: {}", profile.command),
            icon: "custom".to_string(),
            path: resolved_path,
            builtin: false,
            has_profile: true,
        });
    }

    out
}

/// Returns true if at least one AI-category tool was detected.
/// Used by the startup-action resolver to decide whether to show the picker
/// at all, or skip straight to the system shell.
pub fn any_ai_tool_detected(tools: &[DetectedTool]) -> bool {
    tools.iter().any(|t| t.icon == "ai" && t.path.is_some())
}

/// PATH lookup used by detection. Wraps the platform-specific resolver from
/// the `pty` module so detection and spawn share the same search rules
/// (including the augmented per-user dirs like `%APPDATA%\\npm`).
fn which(program: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        crate::pty::which_windows(program)
    }
    #[cfg(not(windows))]
    {
        crate::pty::which_unix(program)
    }
}

#[cfg(windows)]
fn default_shell_path() -> Option<PathBuf> {
    which("pwsh")
        .or_else(|| which("powershell"))
        .or_else(|| which("cmd"))
}

#[cfg(not(windows))]
fn default_shell_path() -> Option<PathBuf> {
    if let Ok(sh) = std::env::var("SHELL") {
        let p = PathBuf::from(&sh);
        if p.is_file() {
            return Some(p);
        }
    }
    which("zsh")
        .or_else(|| which("bash"))
        .or_else(|| which("sh"))
}
