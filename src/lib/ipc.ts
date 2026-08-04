import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface LaunchSpec {
  command: string;
  args: string[];
  cwd: string | null;
  env: Record<string, string>;
  profile: string | null;
}

export interface ProfileSummary {
  name: string;
  command: string;
  args: string[];
}

export interface DetectedTool {
  name: string;
  label: string;
  description: string;
  icon: string;            // "ai" | "terminal" | "custom"
  path: string | null;
  builtin: boolean;
  has_profile: boolean;
}

export type StartupAction =
  | { kind: "launch"; spec: LaunchSpec }
  | { kind: "picker"; detected: DetectedTool[]; no_ai_tools: boolean };

export interface ConfigSnapshot {
  default_profile: string | null;
  ssh_inherit: boolean;
  /**
   * Terminal repaint throttle interval in milliseconds. `0` disables
   * throttling (full-speed repaints). Otherwise output is coalesced and
   * flushed at most once every this many ms (lower GPU usage).
   */
  low_gpu_update_ms: number;
  launcher_order: string[];
  hidden_tools: string[];
  config_path: string | null;
}

export interface ForegroundInfo {
  pid: number;
  name: string;
  cmd: string[];
  cwd: string | null;
  is_ssh: boolean;
}

export interface SpawnPaneResult {
  pane_id: string;
  spec: LaunchSpec;
}

export interface ResolveSplitSpecResult {
  spec: LaunchSpec;
  inherited_ssh: boolean;
  source_foreground: ForegroundInfo | null;
}

export type SpawnSource =
  | { kind: "detected"; name: string }
  | { kind: "profile"; name: string }
  | { kind: "spec"; spec: LaunchSpec };

// Startup / detection ---------------------------------------------------------

export function getStartupAction(): Promise<StartupAction> {
  return invoke("get_startup_action");
}

export function detectTools(): Promise<DetectedTool[]> {
  return invoke("detect_tools");
}

/** Cached detection — fast, no re-scan. Use for the context menu. */
export function getToolsCached(): Promise<DetectedTool[]> {
  return invoke("get_tools_cached");
}

export function listProfiles(): Promise<ProfileSummary[]> {
  return invoke("list_profiles");
}

export function getConfig(): Promise<ConfigSnapshot> {
  return invoke("get_config");
}

export interface VersionInfo {
  semver: string;
  git_hash: string;
  build_date: string;
  display: string;
}

export function getVersion(): Promise<VersionInfo> {
  return invoke("get_version");
}

export function getShellSpec(cwd: string | null): Promise<LaunchSpec> {
  return invoke("get_shell_spec", { cwd });
}

export function setDefaultProfile(name: string | null): Promise<ConfigSnapshot> {
  return invoke("set_default_profile", { args: { name } });
}

export function setSshInherit(enabled: boolean): Promise<ConfigSnapshot> {
  return invoke("set_ssh_inherit", { args: { enabled } });
}

/**
 * Set the terminal repaint throttle interval. `0` disables throttling.
 * Values like 250 / 500 / 1000 cap repaint frequency to that many ms.
 */
export function setLowGpuUpdateMs(lowGpuUpdateMs: number): Promise<ConfigSnapshot> {
  return invoke("set_low_gpu_update_ms", { args: { low_gpu_update_ms: lowGpuUpdateMs } });
}

export function setLauncherOrder(order: string[]): Promise<ConfigSnapshot> {
  return invoke("set_launcher_order", { args: { order } });
}

export function setToolHidden(
  name: string,
  hidden: boolean,
): Promise<ConfigSnapshot> {
  return invoke("set_tool_hidden", { args: { name, hidden } });
}

// PTY -------------------------------------------------------------------------

export function spawnPane(
  source: SpawnSource,
  cols: number,
  rows: number,
): Promise<SpawnPaneResult> {
  return invoke("spawn_pane", { args: { source, cols, rows } });
}

export function writePane(paneId: string, data: string): Promise<void> {
  return invoke("write_pane", { args: { pane_id: paneId, data } });
}

export function resizePane(
  paneId: string,
  cols: number,
  rows: number,
): Promise<void> {
  return invoke("resize_pane", { args: { pane_id: paneId, cols, rows } });
}

export function closePane(paneId: string): Promise<void> {
  return invoke("close_pane", { args: { pane_id: paneId } });
}

export function paneForegroundInfo(
  paneId: string,
): Promise<ForegroundInfo | null> {
  return invoke("pane_foreground_info", { args: { pane_id: paneId } });
}

export function resolveSplitSpec(
  sourcePaneId: string,
  fallbackProfile: string | null,
): Promise<ResolveSplitSpecResult> {
  return invoke("resolve_split_spec", {
    args: {
      source_pane_id: sourcePaneId,
      fallback_profile: fallbackProfile,
    },
  });
}

// Events ----------------------------------------------------------------------

export interface PaneDataEvent {
  pane_id: string;
  chunk: string;
}

export interface PaneExitEvent {
  pane_id: string;
  code: number | null;
}

export function onPaneData(
  cb: (e: PaneDataEvent) => void,
): Promise<UnlistenFn> {
  return listen<PaneDataEvent>("pane:data", (event) => cb(event.payload));
}

export function onPaneExit(
  cb: (e: PaneExitEvent) => void,
): Promise<UnlistenFn> {
  return listen<PaneExitEvent>("pane:exit", (event) => cb(event.payload));
}
