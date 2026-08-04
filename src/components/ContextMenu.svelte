<script lang="ts">
  import { untrack } from "svelte";
  import type { DetectedTool } from "../lib/ipc";
  import {
    WINDOW_SIZE_PRESETS,
    type WindowSizePreset,
  } from "../lib/sizePresets";

  interface Props {
    x: number;
    y: number;
    hasSelection: boolean;
    /** Profile/name of the tool currently running in this pane, if known. */
    currentProfile: string | null;
    /** True when the source pane's foreground process is an ssh client. */
    currentIsSsh: boolean;
    /** Available tools to switch to (from cached detection). */
    availableTools: DetectedTool[];
    onCopy: () => void;
    onPaste: () => void;
    onSplitHorizontal: () => void;
    onSplitVertical: () => void;
    onSwitchTo: (tool: DetectedTool) => void;
    onSetSize: (preset: WindowSizePreset) => void;
    onClose: () => void;
  }

  let {
    x, y,
    hasSelection,
    currentProfile,
    currentIsSsh,
    availableTools,
    onCopy, onPaste,
    onSplitHorizontal, onSplitVertical,
    onSwitchTo,
    onSetSize,
    onClose,
  }: Props = $props();

  let switchOpen = $state(false);
  let sizeOpen = $state(false);

  // --- Smart positioning -----------------------------------------------------
  // The menu is `position: fixed` at the raw click point (x, y). When opened
  // near the window's right or bottom edge, parts of it would render off-screen
  // and be unreachable. After mount we measure the rendered size and shift the
  // menu so the whole thing stays inside the viewport. We also decide whether
  // the submenus should "flip" to open leftward instead of rightward when there
  // isn't room to the right.
  let menuEl = $state<HTMLDivElement | null>(null);
  // Seed with the click point; the $effect below re-clamps on every open.
  // untrack avoids the "captures initial value" lint while still giving us a
  // sane first paint before measurement runs.
  let posX = $state(untrack(() => x));
  let posY = $state(untrack(() => y));
  let submenuFlip = $state(false);
  /** Hidden until we've measured + clamped, to avoid a 1-frame flash at the
   *  raw click point near an edge. */
  let positioned = $state(false);

  /** Minimum clear pixels we want on the right before opening a submenu there. */
  const SUBMENU_CLEARANCE_PX = 200;

  $effect(() => {
    // Re-run whenever the requested anchor changes (a new menu open).
    void x;
    void y;
    const el = menuEl;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const w = rect.width;
    const h = rect.height;

    // Horizontal: shift left if the menu would overflow the right edge.
    let nx = x;
    if (x + w > vw) nx = Math.max(0, vw - w);
    // Vertical: shift up if the menu would overflow the bottom edge.
    let ny = y;
    if (y + h > vh) ny = Math.max(0, vh - h);

    posX = nx;
    posY = ny;

    // Submenu direction: open to the right by default. Flip to the left when
    // there isn't enough clearance between the menu's right edge and the window.
    submenuFlip = vw - (nx + w) < SUBMENU_CLEARANCE_PX;
    positioned = true;
  });

  /**
   * Svelte action applied to each submenu panel: when it opens we clamp it
   * vertically so a tall list (e.g. many size presets) never overflows past the
   * bottom edge. Keeps `top` aligned with its trigger but raises the panel and,
   * if still too tall, makes it scroll.
   */
  function clampSubmenu(node: HTMLElement) {
    const clamp = () => {
      const rect = node.getBoundingClientRect();
      const overflow = rect.bottom - window.innerHeight;
      if (overflow > 0) {
        const newTop = Math.max(0, rect.top - overflow);
        node.style.top = `${newTop}px`;
        // If even pinned to the top it's taller than the viewport, let it scroll.
        node.style.maxHeight = `${Math.max(160, window.innerHeight - newTop - 8)}px`;
        node.style.overflowY = "auto";
      }
    };
    clamp();
    const onResize = () => clamp();
    window.addEventListener("resize", onResize);
    return {
      destroy() {
        window.removeEventListener("resize", onResize);
      },
    };
  }

  /** Tools that can be switched to: all launchable ones. */
  let switchTargets = $derived(
    availableTools.filter((t) => t.name === "shell" || t.path !== null)
  );

  let sizeGroups = $derived(
    WINDOW_SIZE_PRESETS.reduce((groups, preset) => {
      const existing = groups.find((group) => group.name === preset.group);
      if (existing) {
        existing.presets.push(preset);
      } else {
        groups.push({ name: preset.group, presets: [preset] });
      }
      return groups;
    }, [] as { name: WindowSizePreset["group"]; presets: WindowSizePreset[] }[])
  );

  /**
   * Track open state via a counter trick instead of mouseenter/mouseleave on
   * separate elements. The problem with mouseenter/leave on the trigger div is
   * that when the mouse moves to the submenu (which is `position:absolute;
   * left:100%` — outside the trigger's layout box), `mouseleave` fires on the
   * trigger even though the submenu is a DOM child, closing the menu before
   * the user can click. Fix: wrap both trigger and submenu in a parent element
   * and listen there instead.
   */
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
  class="ctx-menu"
  bind:this={menuEl}
  style:left="{posX}px"
  style:top="{posY}px"
  class:submenu-flip={submenuFlip}
  class:hidden={!positioned}
  onclick={(e) => e.stopPropagation()}
  oncontextmenu={(e) => e.preventDefault()}
>
  <!-- Copy / Paste -->
  <button class="item" onclick={onCopy} type="button" disabled={!hasSelection}>
    <span class="icon" aria-hidden="true">
      <svg viewBox="0 0 16 16" width="16" height="16">
        <rect x="4" y="2" width="9" height="11" rx="1.2" fill="none" stroke="currentColor" stroke-width="1.2"/>
        <rect x="2" y="4" width="9" height="11" rx="1.2" fill="var(--menu-bg)" stroke="currentColor" stroke-width="1.2"/>
      </svg>
    </span>
    <span class="label">Copy</span>
    <span class="shortcut">Ctrl+Shift+C</span>
  </button>

  <button class="item" onclick={onPaste} type="button">
    <span class="icon" aria-hidden="true">
      <svg viewBox="0 0 16 16" width="16" height="16">
        <rect x="3" y="3" width="10" height="11" rx="1.2" fill="none" stroke="currentColor" stroke-width="1.2"/>
        <rect x="5.5" y="1.5" width="5" height="2.5" rx="0.6" fill="var(--menu-bg)" stroke="currentColor" stroke-width="1.2"/>
      </svg>
    </span>
    <span class="label">Paste</span>
    <span class="shortcut">Ctrl+Shift+V</span>
  </button>

  <div class="sep"></div>

  <!-- Split -->
  <button class="item" onclick={onSplitHorizontal} type="button">
    <span class="icon" aria-hidden="true">
      <svg viewBox="0 0 16 16" width="16" height="16">
        <rect x="1.5" y="1.5" width="13" height="13" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.2"/>
        <line x1="1.5" y1="8" x2="14.5" y2="8" stroke="currentColor" stroke-width="1.2"/>
      </svg>
    </span>
    <span class="label">{currentIsSsh ? "Dup SSH Horizontal" : "Split Horizontal"}</span>
    <span class="shortcut">Ctrl+Shift+H</span>
  </button>

  <button class="item" onclick={onSplitVertical} type="button">
    <span class="icon" aria-hidden="true">
      <svg viewBox="0 0 16 16" width="16" height="16">
        <rect x="1.5" y="1.5" width="13" height="13" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.2"/>
        <line x1="8" y1="1.5" x2="8" y2="14.5" stroke="currentColor" stroke-width="1.2"/>
      </svg>
    </span>
    <span class="label">{currentIsSsh ? "Dup SSH Vertical" : "Split Vertical"}</span>
    <span class="shortcut">Ctrl+Shift+E</span>
  </button>

  <div class="sep"></div>

  <!-- Switch to → submenu.
       IMPORTANT: the wrapper div captures mouseenter/leave for BOTH the
       trigger row and the submenu panel. This prevents the close-on-gap bug
       where mouseleave fires on the trigger as the pointer moves into the
       absolutely-positioned submenu (which is outside the trigger's layout
       box, but still inside this wrapper's box). -->
  {#if switchTargets.length > 0}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="submenu-wrap"
      onmouseenter={() => (switchOpen = true)}
      onmouseleave={() => (switchOpen = false)}
    >
      <div class="item submenu-trigger" class:open={switchOpen}>
        <span class="icon" aria-hidden="true">
          <svg viewBox="0 0 16 16" width="16" height="16">
            <path d="M3 8h8M8 5l3 3-3 3" fill="none" stroke="currentColor"
              stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>
          </svg>
        </span>
        <span class="label">Switch to</span>
        <span class="arrow">›</span>
      </div>

      {#if switchOpen}
        <div class="submenu" use:clampSubmenu>
          {#each switchTargets as tool (tool.name)}
            <button
              type="button"
              class="item"
              class:current={tool.name === currentProfile}
              onclick={(e) => { e.stopPropagation(); onSwitchTo(tool); }}
            >
              <span class="icon" aria-hidden="true">
                {#if tool.name === currentProfile}
                  <svg viewBox="0 0 16 16" width="16" height="16">
                    <circle cx="8" cy="8" r="3" fill="var(--accent)"/>
                  </svg>
                {:else if tool.icon === "ai"}
                  <svg viewBox="0 0 16 16" width="16" height="16">
                    <path d="M8 2l1.5 3.5 3.5.5-2.5 2.5.5 3.5L8 10.5 5 12l.5-3.5L3 6l3.5-.5z"
                      fill="none" stroke="currentColor" stroke-width="1.1" stroke-linejoin="round"/>
                  </svg>
                {:else}
                  <svg viewBox="0 0 16 16" width="16" height="16">
                    <rect x="2" y="3.5" width="12" height="9" rx="1.5"
                      fill="none" stroke="currentColor" stroke-width="1.1"/>
                    <polyline points="4.5,6.5 7,8.5 4.5,10.5" fill="none"
                      stroke="currentColor" stroke-width="1.1"
                      stroke-linecap="round" stroke-linejoin="round"/>
                  </svg>
                {/if}
              </span>
              <span class="label">{tool.label}</span>
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <div class="sep"></div>
  {/if}

  <!-- Size → submenu -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="submenu-wrap"
    onmouseenter={() => (sizeOpen = true)}
    onmouseleave={() => (sizeOpen = false)}
  >
    <div class="item submenu-trigger" class:open={sizeOpen}>
      <span class="icon" aria-hidden="true">
        <svg viewBox="0 0 16 16" width="16" height="16">
          <rect x="2.5" y="3" width="11" height="10" rx="1.4"
            fill="none" stroke="currentColor" stroke-width="1.2"/>
          <path d="M5 6h6M5 10h6" fill="none" stroke="currentColor"
            stroke-width="1.2" stroke-linecap="round"/>
        </svg>
      </span>
      <span class="label">Size</span>
      <span class="arrow">›</span>
    </div>

    {#if sizeOpen}
      <div class="submenu size-submenu" use:clampSubmenu>
        {#each sizeGroups as group (group.name)}
          <div class="submenu-heading">{group.name}</div>
          {#each group.presets as preset (preset.id)}
            <button
              type="button"
              class="item"
              onclick={(e) => { e.stopPropagation(); onSetSize(preset); }}
            >
              <span class="icon" aria-hidden="true">
                <svg viewBox="0 0 16 16" width="16" height="16">
                  <rect x="3" y="3" width="10" height="10" rx="1.4"
                    fill="none" stroke="currentColor" stroke-width="1.1"/>
                </svg>
              </span>
              <span class="label">{preset.label}</span>
            </button>
          {/each}
        {/each}
      </div>
    {/if}
  </div>

  <div class="sep"></div>

  <!-- Close -->
  <button class="item danger" onclick={onClose} type="button">
    <span class="icon" aria-hidden="true">
      <svg viewBox="0 0 16 16" width="16" height="16">
        <line x1="3" y1="3" x2="13" y2="13" stroke="currentColor" stroke-width="1.4"/>
        <line x1="13" y1="3" x2="3" y2="13" stroke="currentColor" stroke-width="1.4"/>
      </svg>
    </span>
    <span class="label">Close Pane</span>
    <span class="shortcut">Ctrl+Shift+W</span>
  </button>
</div>

<style>
  .ctx-menu {
    position: fixed;
    z-index: 1000;
    background: var(--menu-bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0,0,0,0.4);
    padding: 4px;
    min-width: 220px;
    user-select: none;
  }
  /* Held invisible for one frame while we measure + clamp position so the menu
     never visibly flashes at the raw click point before relocating. */
  .ctx-menu.hidden {
    visibility: hidden;
  }
  .item {
    display: grid;
    grid-template-columns: 20px 1fr auto;
    gap: 8px;
    align-items: center;
    width: 100%;
    padding: 6px 10px;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--fg);
    text-align: left;
    cursor: pointer;
    position: relative;
  }
  .item:hover:not(:disabled),
  .submenu-trigger:hover,
  .submenu-trigger.open {
    background: var(--menu-hover);
  }
  .item:disabled { opacity: 0.4; cursor: default; }
  .item.danger { color: var(--danger); }
  .item.current { color: var(--accent); }
  .icon {
    display: flex; align-items: center; justify-content: center;
    color: var(--fg-dim);
  }
  .item:hover:not(:disabled) .icon,
  .submenu-trigger:hover .icon,
  .submenu-trigger.open .icon { color: inherit; }
  .label { font-size: 13px; }
  .shortcut { font-size: 11px; color: var(--fg-dim); font-variant-numeric: tabular-nums; }
  .arrow { font-size: 14px; color: var(--fg-dim); line-height: 1; }
  .sep { height: 1px; background: var(--border); margin: 4px 0; }

  /* Submenu wrapper — captures hover for both trigger row and panel */
  .submenu-wrap {
    position: relative;
  }
  .submenu-trigger { cursor: default; }
  .submenu-trigger.open,
  .submenu-wrap:hover .submenu-trigger {
    background: var(--menu-hover);
  }
  .submenu {
    position: absolute;
    left: 100%;
    top: 0;
    background: var(--menu-bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0,0,0,0.4);
    padding: 4px;
    min-width: 180px;
    z-index: 1001;
  }
  /* When the menu is anchored near the right edge there's no room for the
     submenu to open to the right, so flip it to open on the left side. */
  .submenu-flip .submenu {
    left: auto;
    right: 100%;
  }
  .size-submenu {
    min-width: 220px;
  }
  .submenu-heading {
    padding: 6px 10px 3px 38px;
    color: var(--fg-dim);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
  }
</style>
