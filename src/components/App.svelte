<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
  import PaneView from "./PaneView.svelte";
  import ContextMenu from "./ContextMenu.svelte";
  import LauncherPicker from "./LauncherPicker.svelte";
  import SettingsPanel from "./SettingsPanel.svelte";
  import {
    closePane,
    getConfig,
    getStartupAction,
    getShellSpec,
    getToolsCached,
    paneForegroundInfo,
    resolveSplitSpec,
    setDefaultProfile,
    spawnPane,
    writePane,
    onPaneData,
    onPaneExit,
    resizePane,
    type DetectedTool,
    type LaunchSpec,
    type PaneDataEvent,
    type PaneExitEvent,
    type SpawnSource,
    type StartupAction,
  } from "../lib/ipc";
  import { copyText, pasteText } from "../lib/clipboard";
  import {
    createInstance,
    destroyInstance,
    focusInstance,
    getSelection,
    noteTerminalUserInput,
    pasteToInstance,
    setTerminalLowGpuUpdateMs,
    writeToInstance,
  } from "../lib/terminalInstances";
  import {
    findLeafByPaneId,
    leaves,
    makeLeaf,
    removeLeaf,
    replaceLeafPaneId,
    setRatio,
    splitLeaf,
    type PaneNode,
    type SplitDirection,
  } from "../lib/PaneTree";
  import type { WindowSizePreset } from "../lib/sizePresets";
  import type { UnlistenFn } from "@tauri-apps/api/event";

  let tree = $state<PaneNode | null>(null);
  let focusedPaneId = $state<string | null>(null);
  let booting = $state(true);
  let bootError = $state<string | null>(null);

  /** Activity: paneIds that have unseen output (not the focused pane). */
  let activePane = $state<Set<string>>(new Set());

  let pickerTools = $state<DetectedTool[] | null>(null);
  let showSettings = $state(false);

  let ctxMenu = $state<{
    x: number;
    y: number;
    paneId: string;
    hasSelection: boolean;
    currentProfile: string | null;
    currentIsSsh: boolean;
  } | null>(null);

  /** Cached tool list for the Switch submenu — loaded once at boot. */
  let availableTools = $state<DetectedTool[]>([]);

  let unlistenData: UnlistenFn | null = null;
  let unlistenExit: UnlistenFn | null = null;

  /**
   * PaneIds currently being deliberately replaced or closed via
   * performSwitch / performClose. handlePaneExit checks this set and skips
   * the shell-respawn for panes we ourselves are killing. Without this,
   * closePane() triggers pane:exit, which fires handlePaneExit, which spawns
   * a shell — racing with and overwriting the tool that performSwitch is
   * about to launch (or resurrecting a pane the user asked to close).
   */
  const switchingPanes = new Set<string>();

  const INITIAL_COLS = 100;
  const INITIAL_ROWS = 30;

  // ---------------------------------------------------------------------------
  // Boot
  // ---------------------------------------------------------------------------

  onMount(async () => {
    // Wire global PTY event listeners before anything is spawned so we never
    // miss the opening burst of output.
    [unlistenData, unlistenExit] = await Promise.all([
      onPaneData(handlePaneData),
      onPaneExit(handlePaneExit),
    ]);

    try {
      const [action, tools] = await Promise.all([
        getStartupAction(),
        getToolsCached(),
      ]);
      const cfg = await getConfig();
      setTerminalLowGpuUpdateMs(cfg.low_gpu_update_ms);
      availableTools = tools;
      if (action.kind === "launch") {
        await spawnFromSpec(action.spec);
      } else {
        pickerTools = action.detected;
      }
    } catch (e) {
      bootError = String(e);
      console.error("[opensplit] boot failed", e);
    } finally {
      booting = false;
    }
  });

  import { onDestroy } from "svelte";
  onDestroy(() => {
    unlistenData?.();
    unlistenExit?.();
  });

  // ---------------------------------------------------------------------------
  // PTY event routing
  // ---------------------------------------------------------------------------

  function handlePaneData(e: PaneDataEvent) {
    writeToInstance(e.pane_id, e.chunk);
    // Mark as active if not the focused pane (activity indicator).
    if (e.pane_id !== focusedPaneId && !activePane.has(e.pane_id)) {
      activePane = new Set([...activePane, e.pane_id]);
    }
  }

  async function handlePaneExit(e: PaneExitEvent) {
    if (!tree) return;
    const exitingLeaf = findLeafByPaneId(tree, e.pane_id);
    if (!exitingLeaf) return;
    // performSwitch deliberately closed this pane — don't respawn a shell.
    if (switchingPanes.has(e.pane_id)) {
      switchingPanes.delete(e.pane_id);
      return;
    }

    // Write the exit notice into the existing terminal (still visible).
    writeToInstance(e.pane_id, `\r\n\x1b[2m[process exited with code ${e.code ?? "?"}]\x1b[0m\r\n`);

    // Respawn as the default shell in the SAME pane position. Inherit cwd from
    // the foreground process if we can detect it; fall back to null (→ $HOME).
    try {
      // Ask backend for the cwd of the dying process's foreground descendant.
      let cwd: string | null = null;
      try {
        const fg = await paneForegroundInfo(e.pane_id);
        cwd = fg?.cwd ?? null;
      } catch { /* best-effort */ }

      const shellSpec = await getShellSpec(cwd);
      const spawned = await spawnPane(
        { kind: "spec", spec: shellSpec },
        INITIAL_COLS,
        INITIAL_ROWS,
      );

      // Create a new xterm instance for the fresh shell pane.
      const respawnId = spawned.pane_id;
      await createInstance(respawnId, (data) => {
        noteTerminalUserInput(respawnId);
        void writePane(respawnId, data).catch(() => {});
      });
      // Re-wire the data/exit listeners are global, so they route automatically.

      // Destroy the old xterm instance (its PTY is already dead).
      destroyInstance(e.pane_id);

      // Update the tree: same leaf node, new paneId + title.
      const newTitle = shellSpec.profile ?? shellSpec.command;
      tree = replaceLeafPaneId(tree!, exitingLeaf.id, spawned.pane_id, newTitle);
      if (focusedPaneId === e.pane_id) focusedPaneId = spawned.pane_id;
    } catch (err) {
      // If respawn fails (e.g. shell not found), leave the dead pane visible
      // with its exit message so the user can read it and close manually.
      console.error("[opensplit] respawn failed", err);
    }
  }

  // ---------------------------------------------------------------------------
  // Spawn helpers
  // ---------------------------------------------------------------------------

  async function spawnFromSpec(spec: LaunchSpec) {
    const result = await spawnPane({ kind: "spec", spec }, INITIAL_COLS, INITIAL_ROWS);
    const title = spec.profile ?? spec.command;
    const paneId = result.pane_id;
    await createInstance(paneId, (data) => {
      noteTerminalUserInput(paneId);
      void writePane(paneId, data).catch(() => {});
    });
    tree = makeLeaf(paneId, spec.profile, title);
    focusedPaneId = paneId;
  }

  async function spawnAndAttachLeaf(source: SpawnSource, title: string): Promise<{ paneId: string }> {
    const result = await spawnPane(source, INITIAL_COLS, INITIAL_ROWS);
    const paneId = result.pane_id;
    await createInstance(paneId, (data) => {
      noteTerminalUserInput(paneId);
      void writePane(paneId, data).catch(() => {});
    });
    return { paneId };
  }

  // ---------------------------------------------------------------------------
  // Picker
  // ---------------------------------------------------------------------------

  async function pickFromLauncher(tool: DetectedTool, setAsDefault: boolean) {
    try {
      if (setAsDefault) {
        try { await setDefaultProfile(tool.name); } catch {}
      }
      pickerTools = null;
      const { paneId } = await spawnAndAttachLeaf(
        { kind: "detected", name: tool.name },
        tool.label,
      );
      tree = makeLeaf(paneId, tool.name, tool.label);
      focusedPaneId = paneId;
    } catch (e) {
      bootError = `Failed to launch ${tool.label}: ${e}`;
      pickerTools = pickerTools ?? [];
    }
  }

  // ---------------------------------------------------------------------------
  // Focus
  // ---------------------------------------------------------------------------

  /**
   * Full focus: user clicked inside a pane to interact with it.
   * Clears the activity dot because they're actively reading output.
   */
  function focusPane(paneId: string) {
    focusedPaneId = paneId;
    activePane = new Set([...activePane].filter((id) => id !== paneId));
    focusInstance(paneId);
  }

  /**
   * Shallow focus: just track which pane owns keyboard/context-menu actions
   * WITHOUT clearing its activity dot. Used by right-click (the user might
   * not have read the output yet — they just want the split menu).
   */
  function setFocusedPane(paneId: string) {
    focusedPaneId = paneId;
    // Deliberately do NOT touch activePane here.
    focusInstance(paneId);
  }

  // ---------------------------------------------------------------------------
  // Context menu
  // ---------------------------------------------------------------------------

  function openContextMenu(ev: MouseEvent, paneId: string) {
    ev.preventDefault();
    ev.stopPropagation();
    setFocusedPane(paneId);
    const hasSelection = getSelection(paneId).length > 0;
    const leaf = tree ? findLeafByPaneId(tree, paneId) : null;
    ctxMenu = {
      x: ev.clientX,
      y: ev.clientY,
      paneId,
      hasSelection,
      currentProfile: leaf?.profile ?? null,
      currentIsSsh: false,
    };

    void paneForegroundInfo(paneId)
      .then((info) => {
        if (!ctxMenu || ctxMenu.paneId !== paneId) return;
        ctxMenu = { ...ctxMenu, currentIsSsh: info?.is_ssh ?? false };
      })
      .catch(() => {});
  }

  function closeContextMenu() {
    ctxMenu = null;
  }

  // ---------------------------------------------------------------------------
  // Split
  // ---------------------------------------------------------------------------

  async function performSplit(sourcePaneId: string, direction: SplitDirection) {
    if (!tree) return;
    const sourceLeaf = findLeafByPaneId(tree, sourcePaneId);
    if (!sourceLeaf) return;

    try {
      const resolved = await resolveSplitSpec(sourcePaneId, sourceLeaf.profile);
      const { paneId } = await spawnAndAttachLeaf(
        { kind: "spec", spec: resolved.spec },
        resolved.inherited_ssh
          ? `ssh: ${resolved.source_foreground?.cmd.slice(1).join(" ") ?? ""}`
          : (resolved.spec.profile ?? resolved.spec.command),
      );

      const newLeaf = makeLeaf(paneId, resolved.spec.profile,
        resolved.inherited_ssh
          ? `ssh: ${resolved.source_foreground?.cmd.slice(1).join(" ") ?? ""}`
          : (resolved.spec.profile ?? resolved.spec.command),
      );
      tree = splitLeaf(tree, sourceLeaf.id, direction, newLeaf);
      focusedPaneId = paneId;
      focusInstance(paneId);
    } catch (e) {
      console.error("[opensplit] split failed", e);
    }
  }

  // ---------------------------------------------------------------------------
  // Switch (replace current pane's process with a different tool)
  // ---------------------------------------------------------------------------

  async function performSwitch(paneId: string, tool: DetectedTool) {
    if (!tree) return;
    const leaf = findLeafByPaneId(tree, paneId);
    if (!leaf) return;

    // Get cwd BEFORE killing the pane (process is still alive at this point).
    let cwd: string | null = null;
    try {
      const fg = await paneForegroundInfo(paneId);
      cwd = fg?.cwd ?? null;
    } catch { /* best-effort */ }

    // Mark as switching BEFORE closePane so handlePaneExit ignores the
    // pane:exit event that closePane triggers.
    switchingPanes.add(paneId);
    try { await closePane(paneId); } catch { switchingPanes.delete(paneId); }
    destroyInstance(paneId);

    try {
      // Use the same spawnAndAttachLeaf path as the picker — it handles
      // profile lookup, detection, and xterm wiring identically to clicking
      // a tile in the launcher picker.
      const source = cwd
        ? { kind: "spec" as const, spec: {
            command: tool.path ?? tool.name,
            args: [] as string[],
            cwd,
            env: {} as Record<string, string>,
            profile: tool.name,
          }}
        : { kind: "detected" as const, name: tool.name };

      const { paneId: newPaneId } = await spawnAndAttachLeaf(source, tool.label);
      tree = replaceLeafPaneId(tree, leaf.id, newPaneId, tool.label);
      focusedPaneId = newPaneId;
      focusInstance(newPaneId);
    } catch (e) {
      console.error("[opensplit] switch failed", e);
    }
  }

  // ---------------------------------------------------------------------------
  // Window size presets
  // ---------------------------------------------------------------------------

  async function performSetWindowSize(preset: WindowSizePreset) {
    try {
      const win = getCurrentWindow();
      if (await win.isMaximized()) {
        await win.unmaximize();
      }
      await win.setSize(new LogicalSize(preset.width, preset.height));
    } catch (e) {
      console.error("[opensplit] set window size failed", e);
    }
  }

  // ---------------------------------------------------------------------------
  // Close
  // ---------------------------------------------------------------------------

  async function performClose(paneId: string) {
    if (!tree) return;
    // Mark as deliberately closed BEFORE closePane so handlePaneExit ignores
    // the pane:exit event the kill triggers. Without this, a fast exit event
    // could race our tree update and respawn a fresh shell over this leaf —
    // making "Close Pane" appear to not work.
    switchingPanes.add(paneId);
    try { await closePane(paneId); } catch { switchingPanes.delete(paneId); }
    destroyInstance(paneId);

    const leaf = findLeafByPaneId(tree, paneId);
    if (!leaf) return;
    const next = removeLeaf(tree, leaf.id);
    tree = next;

    if (next === null) {
      // All panes closed — back to picker or relaunch default.
      try {
        const action = await getStartupAction();
        if (action.kind === "picker") {
          pickerTools = action.detected;
        } else {
          await spawnFromSpec(action.spec);
        }
      } catch {
        await getCurrentWindow().close();
      }
      return;
    }
    const remaining = leaves(next);
    const nextId = remaining[0]?.paneId ?? null;
    focusedPaneId = nextId;
    if (nextId) focusInstance(nextId);
  }

  // ---------------------------------------------------------------------------
  // Splitter drag
  // ---------------------------------------------------------------------------

  function onSplitterDrag(splitId: string, ratio: number) {
    if (!tree) return;
    tree = setRatio(tree, splitId, ratio);
  }

  // ---------------------------------------------------------------------------
  // Copy / Paste
  // ---------------------------------------------------------------------------

  async function performCopy(paneId: string): Promise<boolean> {
    const sel = getSelection(paneId);
    if (!sel) return false;
    try {
      await copyText(sel);
      return true;
    } catch (e) {
      console.warn("[opensplit] clipboard write failed", e);
      return false;
    }
  }

  async function performPaste(paneId: string): Promise<boolean> {
    let text: string;
    try { text = await pasteText(); } catch { return false; }
    if (!text) return false;
    pasteToInstance(paneId, text);
    return true;
  }

  // ---------------------------------------------------------------------------
  // Keyboard
  // ---------------------------------------------------------------------------

  function onKeydown(ev: KeyboardEvent) {
    if (ev.ctrlKey && ev.key === ",") {
      ev.preventDefault();
      showSettings = true;
      return;
    }
    if (ev.ctrlKey && (ev.key === "q" || ev.key === "Q")) {
      ev.preventDefault();
      void getCurrentWindow().close();
      return;
    }
    if (!focusedPaneId) return;
    const mod = ev.ctrlKey && ev.shiftKey;
    if (!mod) return;
    const k = ev.key.toLowerCase();

    if (k === "c") {
      ev.preventDefault(); ev.stopPropagation();
      void performCopy(focusedPaneId);
    } else if (k === "v") {
      ev.preventDefault(); ev.stopPropagation();
      void performPaste(focusedPaneId);
    } else if (k === "h" || ev.key === "-") {
      ev.preventDefault();
      void performSplit(focusedPaneId, "v");
    } else if (k === "e" || ev.key === "|" || ev.key === "\\") {
      ev.preventDefault();
      void performSplit(focusedPaneId, "h");
    } else if (k === "w") {
      ev.preventDefault();
      void performClose(focusedPaneId);
    }
  }
</script>

<svelte:window on:keydown={onKeydown} on:click={closeContextMenu} />

<main class="root">
  {#if booting}
    <div class="boot">Starting OpenSplit…</div>
  {:else if bootError && !pickerTools}
    <div class="boot error">
      <h2>Failed to start</h2>
      <pre>{bootError}</pre>
      <button onclick={() => location.reload()}>Retry</button>
    </div>
  {:else if pickerTools !== null}
    <LauncherPicker detected={pickerTools} onPick={pickFromLauncher} />
  {:else if tree}
    <PaneView
      node={tree}
      {focusedPaneId}
      {activePane}
      onFocus={focusPane}
      onContextMenu={openContextMenu}
      onSplitterDrag={onSplitterDrag}
    />
  {/if}

  {#if !booting}
    <button
      class="gear"
      type="button"
      title="Settings (Ctrl+,)"
      aria-label="Settings"
      onclick={() => (showSettings = true)}
    >
      <svg viewBox="0 0 24 24" width="18" height="18">
        <path
          d="M12 8.5a3.5 3.5 0 1 0 0 7 3.5 3.5 0 0 0 0-7zm8.4 3.5c0 .5-.1 1-.2 1.5l2.1 1.6-2 3.4-2.5-.9c-.7.6-1.6 1-2.5 1.4l-.4 2.6h-4l-.4-2.6c-.9-.3-1.7-.8-2.5-1.4l-2.5.9-2-3.4 2.1-1.6c-.1-.5-.2-1-.2-1.5s.1-1 .2-1.5L3.5 8.9l2-3.4 2.5.9c.8-.6 1.6-1.1 2.5-1.4L10.9 2h4l.4 2.6c.9.3 1.8.8 2.5 1.4l2.5-.9 2 3.4-2.1 1.6c.1.5.2 1 .2 1.5z"
          fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"
        />
      </svg>
    </button>
  {/if}

  {#if showSettings}
    <SettingsPanel onClose={() => (showSettings = false)} />
  {/if}

  {#if ctxMenu}
    <ContextMenu
      x={ctxMenu.x}
      y={ctxMenu.y}
      hasSelection={ctxMenu.hasSelection}
      currentProfile={ctxMenu.currentProfile}
      currentIsSsh={ctxMenu.currentIsSsh}
      {availableTools}
      onCopy={() => { const p = ctxMenu!.paneId; closeContextMenu(); void performCopy(p); }}
      onPaste={() => { const p = ctxMenu!.paneId; closeContextMenu(); void performPaste(p); }}
      onSplitHorizontal={() => { const p = ctxMenu!.paneId; closeContextMenu(); void performSplit(p, "v"); }}
      onSplitVertical={() => { const p = ctxMenu!.paneId; closeContextMenu(); void performSplit(p, "h"); }}
      onSwitchTo={(tool) => { const p = ctxMenu!.paneId; closeContextMenu(); void performSwitch(p, tool); }}
      onSetSize={(preset) => { closeContextMenu(); void performSetWindowSize(preset); }}
      onClose={() => { const p = ctxMenu!.paneId; closeContextMenu(); void performClose(p); }}
    />
  {/if}
</main>

<style>
  .root { position: fixed; inset: 0; background: var(--bg); display: flex; flex-direction: column; }
  .boot { flex: 1; display: flex; align-items: center; justify-content: center; color: var(--fg-dim); font-size: 14px; }
  .boot.error { flex-direction: column; color: var(--danger); padding: 24px; align-items: flex-start; gap: 12px; }
  .boot.error pre { background: var(--bg-elev); padding: 12px; border-radius: 4px; color: var(--fg); max-width: 100%; overflow: auto; white-space: pre-wrap; }
  .gear { position: fixed; top: 8px; right: 8px; z-index: 700; width: 30px; height: 30px; display: flex; align-items: center; justify-content: center; background: rgba(22,22,26,0.7); border: 1px solid var(--border); border-radius: 6px; color: var(--fg-dim); cursor: pointer; padding: 0; backdrop-filter: blur(4px); }
  .gear:hover { color: var(--fg); border-color: var(--border-active); background: var(--bg-elev); }
</style>
