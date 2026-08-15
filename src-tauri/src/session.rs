//! Foreground-process detection + SSH session inheritance.
//!
//! Given a PTY child PID, walk its process tree to find the *currently
//! foregrounded* process inside that PTY. If that process is `ssh`, parse its
//! command line to figure out how to re-launch the same connection in a new
//! pane (ideally reusing the existing OpenSSH ControlMaster socket, which
//! requires no auth).

use std::collections::HashMap;

use serde::Serialize;
use sysinfo::{
    Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind,
};

use crate::config::LaunchSpec;

/// Snapshot of what's running on top of a pane's shell.
#[derive(Debug, Clone, Serialize)]
pub struct ForegroundInfo {
    /// PID of the leaf process (deepest descendant in the pane's tree).
    pub pid: u32,
    /// Basename of the executable (e.g. `ssh`, `bash`, `vim`).
    pub name: String,
    /// Full argv for the leaf process.
    pub cmd: Vec<String>,
    /// Working directory of the leaf process if we can read it. Absent on
    /// platforms / permission setups that don't allow it.
    pub cwd: Option<String>,
    /// True if the leaf looks like an `ssh` client.
    pub is_ssh: bool,
}

/// Detect the foreground process under a PTY-rooted child PID.
///
/// Strategy: build a parent→children map of all processes, then descend from
/// `root_pid` always taking the most-recently-started child. This works
/// reliably for the common cases (shell → app, shell → ssh → remote shell on
/// the local view stops at `ssh`).
pub fn foreground(root_pid: u32) -> Option<ForegroundInfo> {
    // Refresh ONLY the fields read below (name/exe, argv, cwd). The previous
    // `ProcessRefreshKind::everything()` also collected per-process
    // environments and disk counters — the most expensive part of the sweep
    // on Windows (remote PEB reads) — for values nobody used.
    let mut sys = System::new_with_specifics(
        RefreshKind::new().with_processes(
            ProcessRefreshKind::new()
                .with_exe(UpdateKind::Always)
                .with_cmd(UpdateKind::Always)
                .with_cwd(UpdateKind::Always),
        ),
    );
    sys.refresh_processes(ProcessesToUpdate::All, true);

    // children[parent_pid] = Vec<child_pid>
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, proc_) in sys.processes() {
        if let Some(parent) = proc_.parent() {
            children
                .entry(parent.as_u32())
                .or_default()
                .push(pid.as_u32());
        }
    }

    let mut current = root_pid;
    loop {
        let kids = children.get(&current);
        match kids {
            Some(list) if !list.is_empty() => {
                // Take the most recently started child by start_time.
                let mut best = list[0];
                let mut best_start = sys
                    .process(Pid::from_u32(best))
                    .map(|p| p.start_time())
                    .unwrap_or(0);
                for &cand in &list[1..] {
                    let t = sys
                        .process(Pid::from_u32(cand))
                        .map(|p| p.start_time())
                        .unwrap_or(0);
                    if t >= best_start {
                        best = cand;
                        best_start = t;
                    }
                }
                current = best;
            }
            _ => break,
        }
    }

    let proc_ = sys.process(Pid::from_u32(current))?;
    let name = proc_
        .exe()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| proc_.name().to_string_lossy().to_string());
    let cmd: Vec<String> = proc_
        .cmd()
        .iter()
        .map(|s| s.to_string_lossy().to_string())
        .collect();
    let cwd = proc_.cwd().map(|p| p.display().to_string());

    let stem = name.to_lowercase().trim_end_matches(".exe").to_string();
    let is_ssh = stem == "ssh";

    Some(ForegroundInfo {
        pid: current,
        name,
        cmd,
        cwd,
        is_ssh,
    })
}

/// Given the foreground info of the *source* pane and an optional default
/// fallback, build a `LaunchSpec` for the new (split) pane.
///
/// - If the source is `ssh`: re-emit the same `ssh` command, landing on the
///   same remote (ControlMaster reuse when configured, fresh login otherwise).
/// - Otherwise: return the fallback spec, but **inherit the source pane's
///   current working directory** so splits open in the same directory you're
///   already in. This is the #10 quality-of-life fix.
pub fn build_split_spec(source: &ForegroundInfo, fallback: LaunchSpec) -> LaunchSpec {
    if source.is_ssh && !source.cmd.is_empty() {
        let mut cmd = source.cmd.clone();
        let exe = cmd.remove(0);
        return LaunchSpec {
            command: exe,
            args: cmd,
            cwd: source.cwd.clone(),
            env: HashMap::new(),
            profile: Some("ssh-inherit".to_string()),
        };
    }

    // Non-SSH: inherit cwd from foreground process if we were able to read it.
    // The spec's own cwd (if explicitly set) takes priority; we only fill in
    // the gap when it's None.
    LaunchSpec {
        cwd: fallback.cwd.or_else(|| source.cwd.clone()),
        ..fallback
    }
}
