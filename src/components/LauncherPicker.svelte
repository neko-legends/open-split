<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import type { ConfigSnapshot, DetectedTool } from "../lib/ipc";
  import {
    detectTools,
    getConfig,
    setLauncherOrder,
    setToolHidden as saveToolHidden,
  } from "../lib/ipc";

  interface Props {
    detected: DetectedTool[];
    /** Called when the user picks a tool. */
    onPick: (tool: DetectedTool, setAsDefault: boolean) => void;
  }

  interface ToolContextMenu {
    toolName: string;
    hidden: boolean;
    x: number;
    y: number;
  }

  interface PointerDrag {
    toolName: string;
    pointerId: number;
    startX: number;
    startY: number;
    offsetX: number;
    offsetY: number;
    width: number;
    height: number;
    active: boolean;
    startOrder: string[];
  }

  interface DragGhost {
    x: number;
    y: number;
    width: number;
    height: number;
    label: string;
    description: string;
    icon: string;
  }

  interface DragListeners {
    move: (event: PointerEvent) => void;
    up: (event: PointerEvent) => void;
    cancel: (event: PointerEvent) => void;
  }

  interface ToolSlot {
    name: string;
    rect: DOMRect;
    centerX: number;
    centerY: number;
  }

  let { detected, onPick }: Props = $props();

  // Local copy that we can mutate on refresh. We intentionally re-init from
  // the prop on mount (rather than $derived) because once the picker shows
  // the parent doesn't push updates.
  let tools = $state<DetectedTool[]>([]);
  $effect(() => {
    if (tools.length === 0 && detected.length > 0) {
      tools = detected;
    }
  });

  let focusIndex = $state(0);
  let setAsDefault = $state(false);
  let refreshing = $state(false);
  let toolOrder = $state<string[]>([]);
  let hiddenToolNames = $state<string[]>([]);
  let contextMenu = $state<ToolContextMenu | null>(null);
  let draggedToolName = $state<string | null>(null);
  let dragGhost = $state<DragGhost | null>(null);
  let gridEl = $state<HTMLElement | null>(null);

  let pointerDrag: PointerDrag | null = null;
  let dragListeners: DragListeners | null = null;
  let suppressClick = false;

  function isLaunchable(t: DetectedTool): boolean {
    // The "shell" entry is always launchable; otherwise need a resolved path.
    return t.name === "shell" || t.path !== null;
  }

  function uniqueNames(names: string[]): string[] {
    const out: string[] = [];
    for (const name of names) {
      const trimmed = name.trim();
      if (trimmed && !out.includes(trimmed)) {
        out.push(trimmed);
      }
    }
    return out;
  }

  function applyConfigSnapshot(config: ConfigSnapshot) {
    toolOrder = uniqueNames(config.launcher_order);
    hiddenToolNames = uniqueNames(config.hidden_tools);
  }

  function isHidden(name: string): boolean {
    return hiddenToolNames.includes(name);
  }

  function orderedTools(items: DetectedTool[]): DetectedTool[] {
    const orderMap = new Map(toolOrder.map((name, index) => [name, index]));
    return [...items].sort((a, b) => {
      const ai = orderMap.get(a.name);
      const bi = orderMap.get(b.name);
      if (ai === undefined && bi === undefined) return 0;
      if (ai === undefined) return 1;
      if (bi === undefined) return -1;
      return ai - bi;
    });
  }

  /** Ordered view: saved order first, then detection order for new tools. */
  let launchable = $derived(orderedTools(tools.filter(isLaunchable)));
  let visibleLaunchable = $derived(launchable.filter((tool) => !isHidden(tool.name)));
  let hiddenLaunchable = $derived(launchable.filter((tool) => isHidden(tool.name)));
  let contextTool = $derived(
    contextMenu
      ? tools.find((tool) => tool.name === contextMenu?.toolName) ?? null
      : null,
  );

  $effect(() => {
    if (visibleLaunchable.length === 0) {
      focusIndex = 0;
    } else if (focusIndex >= visibleLaunchable.length) {
      focusIndex = visibleLaunchable.length - 1;
    }
  });

  function moveFocus(delta: number) {
    if (visibleLaunchable.length === 0) return;
    focusIndex =
      (focusIndex + delta + visibleLaunchable.length) % visibleLaunchable.length;
  }

  function pickByIndex(i: number) {
    const t = visibleLaunchable[i];
    if (!t) return;
    onPick(t, setAsDefault);
  }

  async function loadConfig() {
    try {
      applyConfigSnapshot(await getConfig());
    } catch (e) {
      console.error("[opensplit] launcher config failed", e);
    }
  }

  async function refresh() {
    refreshing = true;
    try {
      const [nextTools, config] = await Promise.all([detectTools(), getConfig()]);
      tools = nextTools;
      applyConfigSnapshot(config);
    } catch (e) {
      console.error("[opensplit] refresh failed", e);
    } finally {
      refreshing = false;
    }
  }

  function closeContextMenu() {
    contextMenu = null;
  }

  function openToolContextMenu(
    ev: MouseEvent,
    tool: DetectedTool,
    hidden: boolean,
    index: number | null = null,
  ) {
    ev.preventDefault();
    ev.stopPropagation();
    if (index !== null) {
      focusIndex = index;
    }
    contextMenu = {
      toolName: tool.name,
      hidden,
      x: Math.min(ev.clientX, window.innerWidth - 180),
      y: Math.min(ev.clientY, window.innerHeight - 56),
    };
  }

  async function setHidden(name: string, hidden: boolean) {
    const previousHidden = hiddenToolNames;
    hiddenToolNames = hidden
      ? uniqueNames([...hiddenToolNames, name])
      : hiddenToolNames.filter((candidate) => candidate !== name);
    closeContextMenu();

    try {
      applyConfigSnapshot(await saveToolHidden(name, hidden));
    } catch (e) {
      hiddenToolNames = previousHidden;
      console.error("[opensplit] hide/show failed", e);
    }
  }

  function completeOrderForVisible(visibleNames: string[]): string[] {
    const visible = uniqueNames(visibleNames);
    const visibleSet = new Set(visible);
    const orderedNames = launchable.map((tool) => tool.name);
    const hidden = orderedNames.filter(
      (name) => !visibleSet.has(name) && hiddenToolNames.includes(name),
    );
    const hiddenSet = new Set(hidden);
    const rest = orderedNames.filter(
      (name) => !visibleSet.has(name) && !hiddenSet.has(name),
    );
    return [...visible, ...hidden, ...rest];
  }

  async function persistLauncherOrder(nextOrder: string[], fallbackOrder: string[]) {
    try {
      applyConfigSnapshot(await setLauncherOrder(nextOrder));
    } catch (e) {
      toolOrder = fallbackOrder;
      console.error("[opensplit] order save failed", e);
    }
  }

  function toolSlots(): ToolSlot[] {
    const items = Array.from(
      gridEl?.querySelectorAll<HTMLElement>(
        ".tile[data-tool-name], .tile-placeholder[data-tool-name]",
      ) ?? [],
    );

    return items
      .map((item) => {
        const name = item.dataset.toolName ?? "";
        const rect = item.getBoundingClientRect();
        return {
          name,
          rect,
          centerX: rect.left + rect.width / 2,
          centerY: rect.top + rect.height / 2,
        };
      })
      .filter((slot) => slot.name);
  }

  function moveToolNear(sourceName: string, targetName: string, after: boolean): string[] {
    const names = visibleLaunchable.map((tool) => tool.name);
    if (sourceName === targetName) return names;

    const withoutSource = names.filter((name) => name !== sourceName);
    const targetIndex = withoutSource.indexOf(targetName);
    if (targetIndex < 0) return names;

    withoutSource.splice(targetIndex + (after ? 1 : 0), 0, sourceName);
    return withoutSource;
  }

  function visibleNamesForPointer(
    sourceName: string,
    clientX: number,
    clientY: number,
  ): string[] {
    const slots = toolSlots();
    if (slots.length === 0) {
      return visibleLaunchable.map((tool) => tool.name);
    }

    const nearest = slots.reduce<ToolSlot | null>((best, slot) => {
      const dx = clientX - slot.centerX;
      const dy = clientY - slot.centerY;
      const distance = dx * dx + dy * dy * 1.25;
      if (!best) return slot;

      const bestDx = clientX - best.centerX;
      const bestDy = clientY - best.centerY;
      const bestDistance = bestDx * bestDx + bestDy * bestDy * 1.25;
      return distance < bestDistance ? slot : best;
    }, null);

    if (!nearest || nearest.name === sourceName) {
      return visibleLaunchable.map((tool) => tool.name);
    }

    const sameRow = Math.abs(clientY - nearest.centerY) <= nearest.rect.height * 0.35;
    const after = sameRow ? clientX > nearest.centerX : clientY > nearest.centerY;
    return moveToolNear(sourceName, nearest.name, after);
  }

  function dragPosition(
    drag: PointerDrag,
    clientX: number,
    clientY: number,
  ): { x: number; y: number } {
    return {
      x: clientX - drag.offsetX,
      y: clientY - drag.offsetY,
    };
  }

  function dragCenter(
    drag: PointerDrag,
    clientX: number,
    clientY: number,
  ): { x: number; y: number } {
    const position = dragPosition(drag, clientX, clientY);
    return {
      x: position.x + drag.width / 2,
      y: position.y + drag.height / 2,
    };
  }

  function dragGhostForTool(
    toolName: string,
    x: number,
    y: number,
    width: number,
    height: number,
  ): DragGhost | null {
    const tool = launchable.find((candidate) => candidate.name === toolName);
    if (!tool) return null;
    return {
      x,
      y,
      width,
      height,
      label: tool.label,
      description: tool.description,
      icon: tool.icon,
    };
  }

  function clearDrag() {
    if (dragListeners) {
      window.removeEventListener("pointermove", dragListeners.move);
      window.removeEventListener("pointerup", dragListeners.up);
      window.removeEventListener("pointercancel", dragListeners.cancel);
      dragListeners = null;
    }
    pointerDrag = null;
    draggedToolName = null;
    dragGhost = null;
  }

  function previewToolOrder(
    toolName: string,
    clientX: number,
    clientY: number,
  ) {
    const nextVisibleNames = visibleNamesForPointer(toolName, clientX, clientY);
    const currentVisibleNames = visibleLaunchable.map((tool) => tool.name);
    if (nextVisibleNames.join("\0") === currentVisibleNames.join("\0")) return;
    toolOrder = completeOrderForVisible(nextVisibleNames);
  }

  function startPointerDrag(ev: PointerEvent, toolName: string) {
    if (ev.button !== 0) return;
    const tile = (ev.currentTarget as HTMLElement).closest(".tile") as HTMLElement | null;
    if (!tile) return;

    closeContextMenu();
    const rect = tile.getBoundingClientRect();
    pointerDrag = {
      toolName,
      pointerId: ev.pointerId,
      startX: ev.clientX,
      startY: ev.clientY,
      offsetX: ev.clientX - rect.left,
      offsetY: ev.clientY - rect.top,
      width: rect.width,
      height: rect.height,
      active: false,
      startOrder: toolOrder.length
        ? [...toolOrder]
        : launchable.map((tool) => tool.name),
    };
    dragListeners = {
      move: movePointerDrag,
      up: endPointerDrag,
      cancel: (event) => {
        if (pointerDrag?.pointerId === event.pointerId) {
          clearDrag();
        }
      },
    };
    window.addEventListener("pointermove", dragListeners.move);
    window.addEventListener("pointerup", dragListeners.up);
    window.addEventListener("pointercancel", dragListeners.cancel);
  }

  function movePointerDrag(event: PointerEvent) {
    const drag = pointerDrag;
    if (!drag || drag.pointerId !== event.pointerId) return;

    const distance = Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY);
    if (!drag.active && distance > 6) {
      drag.active = true;
      draggedToolName = drag.toolName;
      const position = dragPosition(drag, event.clientX, event.clientY);
      dragGhost = dragGhostForTool(
        drag.toolName,
        position.x,
        position.y,
        drag.width,
        drag.height,
      );
    }

    if (!drag.active) return;
    event.preventDefault();
    const position = dragPosition(drag, event.clientX, event.clientY);
    dragGhost = dragGhost
      ? { ...dragGhost, x: position.x, y: position.y }
      : dragGhostForTool(drag.toolName, position.x, position.y, drag.width, drag.height);

    const center = dragCenter(drag, event.clientX, event.clientY);
    previewToolOrder(drag.toolName, center.x, center.y);
  }

  function endPointerDrag(event: PointerEvent) {
    const drag = pointerDrag;
    if (!drag || drag.pointerId !== event.pointerId) return;

    if (!drag.active) {
      clearDrag();
      return;
    }

    event.preventDefault();
    event.stopPropagation();
    suppressClick = true;
    window.setTimeout(() => {
      suppressClick = false;
    }, 0);

    const center = dragCenter(drag, event.clientX, event.clientY);
    const nextOrder = completeOrderForVisible(
      visibleNamesForPointer(drag.toolName, center.x, center.y),
    );
    toolOrder = nextOrder;
    const fallbackOrder = drag.startOrder;
    clearDrag();
    void persistLauncherOrder(nextOrder, fallbackOrder);
  }

  function keyCameFromControl(ev: KeyboardEvent): boolean {
    const target = ev.target as HTMLElement | null;
    return Boolean(target?.closest("button, input, summary, select, textarea, a"));
  }

  function onKeydown(ev: KeyboardEvent) {
    if (ev.key === "Escape" && contextMenu) {
      ev.preventDefault();
      closeContextMenu();
      return;
    }

    if (keyCameFromControl(ev)) return;
    // Don't trap shortcuts that App.svelte cares about.
    if (ev.ctrlKey || ev.metaKey || ev.altKey) return;

    if (ev.key === "ArrowDown" || ev.key === "ArrowRight") {
      ev.preventDefault();
      moveFocus(1);
    } else if (ev.key === "ArrowUp" || ev.key === "ArrowLeft") {
      ev.preventDefault();
      moveFocus(-1);
    } else if (ev.key === "Enter") {
      ev.preventDefault();
      pickByIndex(focusIndex);
    } else if (/^[1-9]$/.test(ev.key)) {
      const idx = parseInt(ev.key, 10) - 1;
      if (idx < visibleLaunchable.length) {
        ev.preventDefault();
        pickByIndex(idx);
      }
    } else if (ev.key === "d" || ev.key === "D") {
      ev.preventDefault();
      setAsDefault = !setAsDefault;
    } else if (ev.key === "r" || ev.key === "R") {
      ev.preventDefault();
      void refresh();
    }
  }

  onMount(() => {
    void loadConfig();
    window.addEventListener("keydown", onKeydown);
    window.addEventListener("click", closeContextMenu);
    window.addEventListener("resize", closeContextMenu);
  });

  onDestroy(() => {
    window.removeEventListener("keydown", onKeydown);
    window.removeEventListener("click", closeContextMenu);
    window.removeEventListener("resize", closeContextMenu);
    clearDrag();
  });
</script>

<div class="picker">
  <div class="picker-inner">
    <h1 class="title">OpenSplit</h1>
    <p class="subtitle">Pick a tool to launch.</p>

    {#if launchable.length === 0}
      <div class="empty">
        <p>No tools detected on this system.</p>
        <p>
          Install one of: <code>opencode</code>, <code>codex</code>,
          <code>claude</code>, <code>grok-build</code>, <code>hermes</code>,
          <code>aider</code>, etc., or click Refresh.
        </p>
      </div>
    {:else if visibleLaunchable.length === 0}
      <div class="empty">
        <p>All detected tools are hidden.</p>
        <p>Open the Hidden menu below to bring one back.</p>
      </div>
    {/if}

    <div
      class="grid"
      class:dragging-layout={draggedToolName !== null}
      aria-label="Launchable tools"
      bind:this={gridEl}
    >
      {#each visibleLaunchable as tool, i (tool.name)}
        {#if draggedToolName === tool.name}
          <div
            class="tile-placeholder"
            data-tool-name={tool.name}
            aria-hidden="true"
          ></div>
        {:else}
          <div
            class="tile"
            class:focused={i === focusIndex}
            data-tool-name={tool.name}
            role="button"
            tabindex={i === focusIndex ? 0 : -1}
            onclick={() => {
              if (suppressClick) return;
              pickByIndex(i);
            }}
            onkeydown={(ev) => {
              if (ev.key === "Enter" || ev.key === " ") {
                ev.preventDefault();
                ev.stopPropagation();
                pickByIndex(i);
              }
            }}
            oncontextmenu={(ev) => openToolContextMenu(ev, tool, false, i)}
            onmouseenter={() => (focusIndex = i)}
          >
            <span
              class="tile-icon"
              aria-hidden="true"
              title="Drag to reorder"
              onpointerdown={(ev) => startPointerDrag(ev, tool.name)}
            >
              {#if tool.icon === "ai"}
                <svg viewBox="0 0 24 24" width="28" height="28">
                  <rect x="5" y="6" width="14" height="12" rx="3"
                    fill="none" stroke="currentColor" stroke-width="1.4"/>
                  <circle cx="9" cy="12" r="1.2" fill="currentColor"/>
                  <circle cx="15" cy="12" r="1.2" fill="currentColor"/>
                  <path d="M12 3.5v2.5M8.5 18v2M15.5 18v2"
                    fill="none" stroke="currentColor" stroke-width="1.4"
                    stroke-linecap="round"/>
                </svg>
              {:else if tool.icon === "terminal"}
                <svg viewBox="0 0 24 24" width="28" height="28">
                  <rect x="2.5" y="4.5" width="19" height="15" rx="2"
                    fill="none" stroke="currentColor" stroke-width="1.4"/>
                  <polyline points="6,9 10,12 6,15" fill="none"
                    stroke="currentColor" stroke-width="1.4"
                    stroke-linecap="round" stroke-linejoin="round"/>
                  <line x1="12" y1="15" x2="17" y2="15"
                    stroke="currentColor" stroke-width="1.4"
                    stroke-linecap="round"/>
                </svg>
              {:else}
                <svg viewBox="0 0 24 24" width="28" height="28">
                  <circle cx="12" cy="12" r="9" fill="none"
                    stroke="currentColor" stroke-width="1.4"/>
                  <path d="M8 12l3 3 5-6" fill="none"
                    stroke="currentColor" stroke-width="1.4"
                    stroke-linecap="round" stroke-linejoin="round"/>
                </svg>
              {/if}
            </span>
            <span class="tile-label">{tool.label}</span>
            <span class="tile-desc">{tool.description}</span>
            <span class="tile-actions">
              {#if i < 9}
                <span class="tile-key">{i + 1}</span>
              {/if}
              <button
                type="button"
                class="hide-button"
                aria-label={`Hide ${tool.label}`}
                title="Hide"
                onpointerdown={(ev) => ev.stopPropagation()}
                onclick={(ev) => {
                  ev.stopPropagation();
                  void setHidden(tool.name, true);
                }}
              >
                <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
                  <path d="M2 8s2-4 6-4 6 4 6 4-2 4-6 4-6-4-6-4z"
                    fill="none" stroke="currentColor" stroke-width="1.2"
                    stroke-linejoin="round"/>
                  <line x1="3" y1="13" x2="13" y2="3"
                    stroke="currentColor" stroke-width="1.3"
                    stroke-linecap="round"/>
                </svg>
              </button>
            </span>
          </div>
        {/if}
      {/each}
    </div>

    <div class="footer">
      <label class="checkbox">
        <input type="checkbox" bind:checked={setAsDefault} />
        <span>Remember this as my default <kbd>D</kbd></span>
      </label>

      <details class="hidden-dropdown">
        <summary>Hidden ({hiddenLaunchable.length})</summary>
        <div class="hidden-list">
          {#if hiddenLaunchable.length === 0}
            <span class="hidden-empty">No hidden tools</span>
          {:else}
            {#each hiddenLaunchable as tool (tool.name)}
              <button
                type="button"
                class="hidden-item"
                data-tool-name={tool.name}
                oncontextmenu={(ev) => openToolContextMenu(ev, tool, true)}
                onclick={() => void setHidden(tool.name, false)}
              >
                <span>{tool.label}</span>
                <small>Show</small>
              </button>
            {/each}
          {/if}
        </div>
      </details>

      <div class="footer-spacer"></div>
      <button
        type="button"
        class="ghost"
        onclick={refresh}
        disabled={refreshing}
      >
        {refreshing ? "Scanning…" : "Refresh"} <kbd>R</kbd>
      </button>
    </div>

    <p class="hint">
      <kbd>↑</kbd><kbd>↓</kbd> navigate · <kbd>Enter</kbd> select ·
      <kbd>1</kbd>–<kbd>9</kbd> quick pick
    </p>
  </div>

  {#if contextMenu && contextTool}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="tool-menu"
      style={`left: ${contextMenu.x}px; top: ${contextMenu.y}px;`}
      onclick={(ev) => ev.stopPropagation()}
      oncontextmenu={(ev) => ev.preventDefault()}
    >
      <button
        type="button"
        onclick={() => void setHidden(contextTool!.name, !contextMenu!.hidden)}
      >
        {contextMenu.hidden ? "Show in launcher" : "Hide from launcher"}
      </button>
    </div>
  {/if}

  {#if dragGhost}
    <div
      class="drag-ghost"
      class:terminal-ghost={dragGhost.icon === "terminal"}
      style={`left: ${dragGhost.x}px; top: ${dragGhost.y}px; width: ${dragGhost.width}px; min-height: ${dragGhost.height}px;`}
      aria-hidden="true"
    >
      <span class="ghost-icon">
        {dragGhost.icon === "terminal" ? ">" : "AI"}
      </span>
      <span class="ghost-copy">
        <strong>{dragGhost.label}</strong>
        <small>{dragGhost.description}</small>
      </span>
    </div>
  {/if}
</div>

<style>
  .picker {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--bg);
    overflow: auto;
    padding: 24px;
  }
  .picker-inner {
    width: 100%;
    max-width: 760px;
  }
  .title {
    font-size: 28px;
    font-weight: 600;
    margin: 0 0 4px;
    color: var(--fg);
    text-align: center;
  }
  .subtitle {
    margin: 0 0 28px;
    color: var(--fg-dim);
    text-align: center;
    font-size: 14px;
  }
  .empty {
    background: var(--bg-elev);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 20px;
    margin: 0 0 20px;
    color: var(--fg-dim);
    text-align: center;
  }
  .empty code {
    background: var(--bg);
    padding: 1px 5px;
    border-radius: 3px;
    color: var(--fg);
    font-size: 12px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 12px;
    margin-bottom: 20px;
  }
  .tile,
  .tile-placeholder {
    position: relative;
    min-height: 126px;
    border-radius: 8px;
  }
  .tile {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    padding: 14px 48px 14px 16px;
    background: var(--bg-elev);
    border: 1px solid var(--border);
    text-align: left;
    cursor: pointer;
    transition: background 80ms ease, border-color 80ms ease, transform 80ms ease;
    color: var(--fg);
  }
  .tile:hover {
    background: var(--menu-hover);
  }
  .tile.focused {
    border-color: var(--border-active);
    background: var(--menu-hover);
  }
  .tile:active {
    transform: scale(0.99);
  }
  .dragging-layout .tile:hover,
  .dragging-layout .tile.focused {
    transform: none;
  }
  .tile-placeholder {
    border: 1px dashed var(--border-active);
    background: rgba(74, 144, 226, 0.08);
  }
  .tile-icon {
    color: var(--accent);
    line-height: 0;
    cursor: grab;
    touch-action: none;
  }
  .tile-icon:active {
    cursor: grabbing;
  }
  .tile-label {
    font-size: 15px;
    font-weight: 600;
  }
  .tile-desc {
    font-size: 12px;
    color: var(--fg-dim);
    line-height: 1.35;
  }
  .tile-actions {
    position: absolute;
    top: 10px;
    right: 10px;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .tile-key {
    font-size: 10px;
    color: var(--fg-dim);
    background: var(--bg);
    border: 1px solid var(--border);
    padding: 1px 5px;
    border-radius: 3px;
    font-variant-numeric: tabular-nums;
  }
  .hide-button {
    width: 24px;
    height: 24px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0;
    border-radius: 5px;
    color: var(--fg-dim);
    background: var(--bg);
  }
  .hide-button:hover {
    color: var(--fg);
    background: var(--menu-hover);
  }
  .footer {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 8px;
  }
  .footer-spacer {
    flex: 1;
  }
  .checkbox {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--fg-dim);
    font-size: 12px;
    cursor: pointer;
  }
  .checkbox input {
    accent-color: var(--accent);
  }
  .hidden-dropdown {
    position: relative;
    color: var(--fg);
    font-size: 12px;
  }
  .hidden-dropdown summary {
    list-style: none;
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 6px 10px;
    cursor: pointer;
    background: transparent;
  }
  .hidden-dropdown summary::-webkit-details-marker {
    display: none;
  }
  .hidden-dropdown[open] summary,
  .hidden-dropdown summary:hover {
    border-color: var(--border-active);
    background: var(--menu-hover);
  }
  .hidden-list {
    position: absolute;
    left: 0;
    bottom: calc(100% + 8px);
    z-index: 20;
    min-width: 190px;
    max-height: 220px;
    overflow: auto;
    padding: 6px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--menu-bg);
    box-shadow: 0 12px 36px rgba(0, 0, 0, 0.42);
  }
  .hidden-empty {
    display: block;
    padding: 8px;
    color: var(--fg-dim);
  }
  .hidden-item {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 7px 8px;
    border: 0;
    border-radius: 5px;
    text-align: left;
    background: transparent;
  }
  .hidden-item small {
    color: var(--fg-dim);
    font-size: 10px;
  }
  .hidden-item:hover {
    background: var(--menu-hover);
  }
  .ghost {
    background: transparent;
    border: 1px solid var(--border);
  }
  .ghost:hover:not(:disabled) {
    border-color: var(--border-active);
    background: var(--menu-hover);
  }
  .ghost:disabled {
    opacity: 0.6;
    cursor: progress;
  }
  .hint {
    text-align: center;
    color: var(--fg-dim);
    font-size: 11px;
    margin: 4px 0 0;
  }
  .tool-menu {
    position: fixed;
    z-index: 60;
    min-width: 166px;
    padding: 5px;
    background: var(--menu-bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 14px 36px rgba(0, 0, 0, 0.46);
  }
  .tool-menu button {
    width: 100%;
    border: 0;
    border-radius: 5px;
    padding: 8px 9px;
    text-align: left;
    background: transparent;
  }
  .tool-menu button:hover {
    background: var(--menu-hover);
  }
  .drag-ghost {
    position: fixed;
    z-index: 70;
    pointer-events: none;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 10px;
    max-width: calc(100vw - 32px);
    padding: 12px;
    border: 1px solid var(--border-active);
    border-radius: 8px;
    background: color-mix(in srgb, var(--bg-elev) 92%, var(--accent));
    box-shadow: 0 18px 44px rgba(0, 0, 0, 0.5);
    opacity: 0.96;
  }
  .ghost-icon {
    width: 34px;
    height: 34px;
    border-radius: 7px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--accent);
    border: 1px solid var(--border);
    font-size: 11px;
    font-weight: 700;
  }
  .terminal-ghost .ghost-icon {
    font-family: "Cascadia Code", monospace;
  }
  .ghost-copy {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .ghost-copy strong,
  .ghost-copy small {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ghost-copy small {
    color: var(--fg-dim);
    font-size: 11px;
  }
  kbd {
    background: var(--bg-elev);
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 1px 4px;
    font-size: 10px;
    font-family: inherit;
    color: var(--fg);
  }
</style>
