<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import type { ConfigSnapshot, DetectedTool, VersionInfo } from "../lib/ipc";
  import {
    detectTools,
    getConfig,
    getVersion,
    setDefaultProfile,
    setLowGpuUpdateMs,
    setSshInherit,
  } from "../lib/ipc";
  import { setTerminalLowGpuUpdateMs } from "../lib/terminalInstances";

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  let config = $state<ConfigSnapshot | null>(null);
  let tools = $state<DetectedTool[]>([]);
  let version = $state<VersionInfo | null>(null);
  let loading = $state(true);
  let refreshing = $state(false);
  let error = $state<string | null>(null);

  async function loadAll() {
    loading = true;
    error = null;
    try {
      const [c, t, v] = await Promise.all([getConfig(), detectTools(), getVersion()]);
      config = c;
      setTerminalLowGpuUpdateMs(c.low_gpu_update_ms);
      tools = t;
      version = v;
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function refresh() {
    refreshing = true;
    try {
      tools = await detectTools();
    } catch (e) {
      error = String(e);
    } finally {
      refreshing = false;
    }
  }

  async function pickDefault(name: string | null) {
    try {
      config = await setDefaultProfile(name);
    } catch (e) {
      error = String(e);
    }
  }

  async function toggleSshInherit() {
    if (!config) return;
    try {
      config = await setSshInherit(!config.ssh_inherit);
    } catch (e) {
      error = String(e);
    }
  }

  /**
   * Available repaint throttle intervals. `0` disables throttling (full-speed
   * repaints); the others coalesce output and repaint at most that often, which
   * lowers GPU/composite load on lower-end hardware.
   */
  const RENDER_INTERVAL_OPTIONS: { value: number; label: string }[] = [
    { value: 0, label: "Off (full speed)" },
    { value: 250, label: "0.25s" },
    { value: 500, label: "0.5s" },
    { value: 1000, label: "1s" },
  ];

  async function changeRenderInterval(ev: Event) {
    if (!config) return;
    const value = Number((ev.target as HTMLSelectElement).value);
    try {
      config = await setLowGpuUpdateMs(value);
      setTerminalLowGpuUpdateMs(config.low_gpu_update_ms);
    } catch (e) {
      error = String(e);
    }
  }

  function onKeydown(ev: KeyboardEvent) {
    if (ev.key === "Escape") {
      ev.preventDefault();
      onClose();
    }
  }

  onMount(() => {
    void loadAll();
    window.addEventListener("keydown", onKeydown);
  });

  onDestroy(() => {
    window.removeEventListener("keydown", onKeydown);
  });
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="overlay" onclick={onClose}>
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="panel" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-label="Settings" tabindex="-1">
    <header class="header">
      <h2>Settings</h2>
      <button class="close" type="button" onclick={onClose} aria-label="Close">
        <svg viewBox="0 0 16 16" width="16" height="16">
          <line x1="3" y1="3" x2="13" y2="13" stroke="currentColor" stroke-width="1.4"/>
          <line x1="13" y1="3" x2="3" y2="13" stroke="currentColor" stroke-width="1.4"/>
        </svg>
      </button>
    </header>

    {#if loading}
      <div class="state">Loading…</div>
    {:else if error}
      <div class="state error">{error}</div>
    {:else if config}
      <section class="section">
        <div class="section-head">
          <h3>Default tool on launch</h3>
          <button
            class="ghost small"
            type="button"
            onclick={refresh}
            disabled={refreshing}
          >
            {refreshing ? "Scanning…" : "Refresh detection"}
          </button>
        </div>
        <p class="hint">
          Used when you type <code>opensplit</code> with no arguments.
          Pick <em>Show picker</em> to always see the launcher.
        </p>

        <div class="options">
          <button
            type="button"
            class="opt"
            class:selected={config.default_profile === null}
            onclick={() => pickDefault(null)}
          >
            <span class="opt-label">Show picker on launch</span>
            <span class="opt-desc">
              Always ask which tool to start (current behavior when no default).
            </span>
          </button>

          {#each tools.filter((t) => t.name === "shell" || t.path) as tool (tool.name)}
            <button
              type="button"
              class="opt"
              class:selected={config.default_profile === tool.name}
              onclick={() => pickDefault(tool.name)}
            >
              <span class="opt-label">{tool.label}</span>
              <span class="opt-desc">
                {tool.description}
                {#if tool.path}
                  <span class="path">{tool.path}</span>
                {/if}
              </span>
            </button>
          {/each}
        </div>

        {#if tools.filter((t) => t.path).length === 0}
          <p class="hint dim">
            No AI CLIs detected on PATH. Install something like
            <code>opencode</code>, <code>codex</code>, or <code>claude</code>
            and click Refresh.
          </p>
        {/if}
      </section>

      <section class="section">
        <h3>SSH inheritance</h3>
        <label class="toggle">
          <input
            type="checkbox"
            checked={config.ssh_inherit}
            onchange={toggleSshInherit}
          />
          <span>
            When splitting an SSH pane, re-run the same connection in the new
            pane (reuses OpenSSH ControlMaster when configured).
          </span>
        </label>
      </section>

      <section class="section">
        <h3>Rendering</h3>
        <div class="select-row">
          <label class="select-label" for="render-interval">Update rate</label>
          <select
            id="render-interval"
            class="select"
            value={config.low_gpu_update_ms}
            onchange={changeRenderInterval}
          >
            {#each RENDER_INTERVAL_OPTIONS as opt (opt.value)}
              <option value={opt.value}>{opt.label}</option>
            {/each}
          </select>
        </div>
        <p class="hint">
          Caps how often terminal output repaints. Lower intervals feel more
          responsive but use more GPU; higher intervals reduce GPU load. Has no
          effect while a pane is actively receiving your keystrokes.
        </p>
      </section>

      <section class="section">
        <h3>Config file</h3>
        <p class="hint">
          <code class="filepath">{config.config_path ?? "(unknown)"}</code>
        </p>
      </section>

      {#if version}
        <section class="section version-section">
          <span class="version-str">OpenSplit {version.display}</span>
        </section>
      {/if}
    {/if}
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 800;
  }
  .panel {
    width: min(680px, calc(100vw - 40px));
    max-height: calc(100vh - 40px);
    overflow: auto;
    background: var(--bg-elev);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.5);
    color: var(--fg);
  }
  .header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 18px;
    border-bottom: 1px solid var(--border);
  }
  .header h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }
  .close {
    background: transparent;
    border: none;
    color: var(--fg-dim);
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
  }
  .close:hover {
    background: var(--menu-hover);
    color: var(--fg);
  }
  .section {
    padding: 16px 18px;
    border-bottom: 1px solid var(--border);
  }
  .section:last-child {
    border-bottom: none;
  }
  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 4px;
  }
  .section h3 {
    margin: 0 0 4px;
    font-size: 13px;
    font-weight: 600;
    color: var(--fg);
  }
  .hint {
    margin: 4px 0 12px;
    font-size: 12px;
    color: var(--fg-dim);
  }
  .hint.dim {
    margin-top: 12px;
  }
  .hint code,
  .filepath {
    background: var(--bg);
    padding: 2px 5px;
    border-radius: 3px;
    color: var(--fg);
    font-family: 'Cascadia Code', monospace;
    font-size: 11px;
  }
  .filepath {
    user-select: all;
  }
  .options {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .opt {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding: 10px 12px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    text-align: left;
    cursor: pointer;
    color: var(--fg);
  }
  .opt:hover {
    background: var(--menu-hover);
    border-color: var(--border-active);
  }
  .opt.selected {
    border-color: var(--border-active);
    background: var(--menu-hover);
  }
  .opt.selected::before {
    content: "●";
    position: absolute;
    margin-left: -18px;
    color: var(--accent);
  }
  .opt {
    position: relative;
    padding-left: 24px;
  }
  .opt-label {
    font-size: 13px;
    font-weight: 500;
  }
  .opt-desc {
    font-size: 11px;
    color: var(--fg-dim);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .path {
    font-family: 'Cascadia Code', monospace;
    font-size: 10px;
    color: var(--fg-dim);
    opacity: 0.7;
  }
  .toggle {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    cursor: pointer;
    color: var(--fg);
    font-size: 12px;
  }
  .toggle input {
    margin-top: 2px;
    accent-color: var(--accent);
  }
  .select-row {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .select-label {
    font-size: 12px;
    color: var(--fg);
  }
  .select {
    background: var(--bg);
    color: var(--fg);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 5px 8px;
    font-size: 12px;
    font-family: inherit;
    cursor: pointer;
    min-width: 140px;
  }
  .select:hover {
    border-color: var(--border-active);
  }
  .select:focus {
    outline: none;
    border-color: var(--accent);
  }
  .ghost {
    background: transparent;
    border: 1px solid var(--border);
    color: var(--fg);
    padding: 6px 10px;
    border-radius: 5px;
    font-size: 12px;
    cursor: pointer;
  }
  .ghost.small {
    padding: 4px 8px;
    font-size: 11px;
  }
  .ghost:hover:not(:disabled) {
    border-color: var(--border-active);
    background: var(--menu-hover);
  }
  .ghost:disabled {
    opacity: 0.6;
    cursor: progress;
  }
  .state {
    padding: 24px;
    text-align: center;
    color: var(--fg-dim);
  }
  .state.error {
    color: var(--danger);
  }
  .version-section {
    display: flex;
    justify-content: flex-end;
    padding: 10px 18px;
  }
  .version-str {
    font-size: 11px;
    color: var(--fg-dim);
    font-family: 'Cascadia Code', monospace;
    opacity: 0.7;
  }
</style>
