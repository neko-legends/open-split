/**
 * Persistent xterm.js instances keyed by paneId.
 *
 * The fundamental problem we're solving: Svelte's recursive PaneView tree
 * mounts/unmounts Terminal components whenever the tree shape changes (e.g.
 * on a split a leaf is replaced by a Split node, destroying the old component
 * and creating a new one for the same paneId). The PTY on the backend survives
 * but the local xterm scrollback, cursor position, and rendered history are
 * gone.
 *
 * Fix: Terminal components don't OWN an xterm instance — they BORROW one from
 * this registry. The xterm instance is created once per paneId and lives until
 * the pane is explicitly closed. Components attach/detach by appending/removing
 * the xterm DOM element from their host div.
 *
 * Lazy-load bonus: we import xterm only when the first terminal actually needs
 * to mount, so the picker screen (which doesn't need a terminal at all) loads
 * ~0 KB of xterm until necessary.
 */

import type { Terminal } from "@xterm/xterm";
import type { FitAddon } from "@xterm/addon-fit";
import { copyText } from "./clipboard";

export interface TerminalInstance {
  term: Terminal;
  fitAddon: FitAddon;
  /** The root element produced by term.open(); we move this between host divs. */
  element: HTMLElement;
  paneId: string;
  /** Unprocessed output that arrived before the first host attached. */
  earlyChunks: string[];
  /** Output waiting to be flushed into xterm. Coalesced to cap paint churn. */
  pendingChunks: string[];
  pendingCharCount: number;
  flushTimer: number;
  lastFlushAt: number;
  interactiveUntil: number;
  /** Whether this instance has been opened into a DOM element yet. */
  opened: boolean;
}

const instances = new Map<string, TerminalInstance>();

const XTERM_THEME = {
  background: "#0e0e10",
  foreground: "#e6e6e8",
  cursor: "#e6e6e8",
  cursorAccent: "#0e0e10",
  selectionBackground: "#4a90e2aa",
  black: "#16161a",
  red: "#e25c5c",
  green: "#7bc47f",
  yellow: "#e2c25c",
  blue: "#4a90e2",
  magenta: "#c25ce2",
  cyan: "#5ce2c2",
  white: "#e6e6e8",
  brightBlack: "#5a5a65",
  brightRed: "#ff7b7b",
  brightGreen: "#9be4a0",
  brightYellow: "#ffd97b",
  brightBlue: "#6ea9ee",
  brightMagenta: "#d77bee",
  brightCyan: "#7beedb",
  brightWhite: "#ffffff",
};

const OSC52_CLIPBOARD_TARGETS = new Set(["c", "p", "s", "0", "1", "2", "3", "4", "5", "6", "7"]);
const OSC52_MAX_DECODED_BYTES = 4 * 1024 * 1024;
const VISIBLE_OUTPUT_FLUSH_MS = 66;
const INTERACTIVE_OUTPUT_FLUSH_MS = 16;
const INTERACTIVE_GRACE_MS = 500;
const HIDDEN_OUTPUT_FLUSH_MS = 250;
const MAX_PENDING_OUTPUT_CHARS = 4 * 1024 * 1024;
let visibilityFlushInstalled = false;
/**
 * User-configured repaint throttle interval in ms. `0` disables throttling
 * (full-speed repaints via the visible/hidden intervals). Otherwise output is
 * coalesced and flushed at most once every this many ms — reduces GPU load.
 */
let lowGpuUpdateMs = 0;

function decodeOsc52Payload(data: string): string | null {
  const semicolon = data.indexOf(";");
  if (semicolon === -1) return null;

  const target = data.slice(0, semicolon);
  const encoded = data.slice(semicolon + 1);
  if (!OSC52_CLIPBOARD_TARGETS.has(target) || !encoded || encoded === "?") {
    return null;
  }

  const normalized = encoded.replace(/\s/g, "");
  const decodedLength = Math.floor((normalized.length * 3) / 4);
  if (decodedLength > OSC52_MAX_DECODED_BYTES) {
    console.warn("[opensplit] ignored oversized OSC 52 clipboard payload");
    return null;
  }

  try {
    const binary = atob(normalized);
    const bytes = Uint8Array.from(binary, (ch) => ch.charCodeAt(0));
    return new TextDecoder().decode(bytes);
  } catch (e) {
    console.warn("[opensplit] failed to decode OSC 52 clipboard payload", e);
    return null;
  }
}

function installOsc52ClipboardHandler(term: Terminal): void {
  term.parser.registerOscHandler(52, (data) => {
    const text = decodeOsc52Payload(data);
    if (text === null) return true;

    void copyText(text).catch((e) => {
      console.warn("[opensplit] failed to write OSC 52 clipboard payload", e);
    });
    return true;
  });
}

function outputFlushIntervalMs(): number {
  if (lowGpuUpdateMs > 0) return lowGpuUpdateMs;
  return document.visibilityState === "hidden"
    ? HIDDEN_OUTPUT_FLUSH_MS
    : VISIBLE_OUTPUT_FLUSH_MS;
}

function instanceFlushIntervalMs(inst: TerminalInstance): number {
  // While throttled, briefly speed up after user keystrokes so typing still
  // feels responsive, then fall back to the throttle interval.
  if (lowGpuUpdateMs > 0 && performance.now() < inst.interactiveUntil) {
    return INTERACTIVE_OUTPUT_FLUSH_MS;
  }
  return outputFlushIntervalMs();
}

function flushPendingOutput(inst: TerminalInstance): void {
  if (inst.flushTimer) {
    window.clearTimeout(inst.flushTimer);
    inst.flushTimer = 0;
  }
  if (!inst.opened || inst.pendingChunks.length === 0) return;

  const chunk = inst.pendingChunks.length === 1
    ? inst.pendingChunks[0]
    : inst.pendingChunks.join("");
  inst.pendingChunks.length = 0;
  inst.pendingCharCount = 0;
  inst.lastFlushAt = performance.now();

  try {
    inst.term.write(chunk);
  } catch {
    /* instance may be mid-destroy; ignore */
  }
}

function scheduleOutputFlush(inst: TerminalInstance): void {
  if (inst.flushTimer || !inst.opened) return;

  const elapsed = performance.now() - inst.lastFlushAt;
  const delay = Math.max(0, instanceFlushIntervalMs(inst) - elapsed);
  inst.flushTimer = window.setTimeout(() => {
    inst.flushTimer = 0;
    flushPendingOutput(inst);
  }, delay);
}

function queueOutput(inst: TerminalInstance, chunk: string): void {
  inst.pendingChunks.push(chunk);
  inst.pendingCharCount += chunk.length;

  if (inst.pendingCharCount >= MAX_PENDING_OUTPUT_CHARS) {
    flushPendingOutput(inst);
  } else {
    scheduleOutputFlush(inst);
  }
}

function installVisibilityFlush(): void {
  if (visibilityFlushInstalled) return;
  visibilityFlushInstalled = true;
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible") return;
    for (const inst of instances.values()) {
      flushPendingOutput(inst);
    }
  });
}

/**
 * Update the repaint throttle interval. Pass `0` to disable throttling
 * (full-speed repaints). Any positive value caps repaint frequency to that
 * many ms between flushes. When throttling is turned off we immediately flush
 * any buffered output so nothing is left waiting.
 */
export function setTerminalLowGpuUpdateMs(intervalMs: number): void {
  lowGpuUpdateMs = Math.max(0, Math.floor(intervalMs));
  if (lowGpuUpdateMs === 0) {
    for (const inst of instances.values()) {
      flushPendingOutput(inst);
    }
  }
}

export function noteTerminalUserInput(paneId: string): void {
  const inst = instances.get(paneId);
  if (!inst) return;
  inst.interactiveUntil = performance.now() + INTERACTIVE_GRACE_MS;

  if (inst.pendingChunks.length > 0) {
    flushPendingOutput(inst);
  }
}

/**
 * Create a new persistent xterm instance for a pane. Call once per paneId
 * (typically right after spawn_pane succeeds). The instance is NOT yet opened
 * into the DOM — that happens in `attach()`.
 *
 * We lazy-import xterm here so this only runs when a pane is actually spawned,
 * not during the picker screen.
 */
export async function createInstance(
  paneId: string,
  onData: (data: string) => void,
): Promise<TerminalInstance> {
  installVisibilityFlush();

  // Lazy imports — only load xterm when we actually need it.
  const [{ Terminal }, { FitAddon }, { WebLinksAddon }] = await Promise.all([
    import("@xterm/xterm"),
    import("@xterm/addon-fit"),
    import("@xterm/addon-web-links"),
  ]);
  // CSS is side-effect only; import once.
  await import("@xterm/xterm/css/xterm.css");

  const term = new Terminal({
    fontFamily:
      "'Cascadia Code', 'JetBrains Mono', 'Fira Code', Menlo, Consolas, 'Liberation Mono', monospace",
    fontSize: 13,
    lineHeight: 1.15,
    cursorBlink: false,
    cursorStyle: "block",
    allowTransparency: false,
    scrollback: 5000,
    theme: XTERM_THEME,
  });

  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);
  term.loadAddon(new WebLinksAddon());
  installOsc52ClipboardHandler(term);
  term.onData(onData);

  // We don't call term.open() yet because we need a real host element.
  const inst: TerminalInstance = {
    term,
    fitAddon,
    element: null as unknown as HTMLElement, // set in attach()
    paneId,
    earlyChunks: [],
    pendingChunks: [],
    pendingCharCount: 0,
    flushTimer: 0,
    lastFlushAt: 0,
    interactiveUntil: 0,
    opened: false,
  };

  instances.set(paneId, inst);
  return inst;
}

/**
 * Attach (or re-attach) a persistent instance to a new host element.
 *
 * On first call: term.open(host) creates the xterm canvas inside `host`.
 * On subsequent calls (re-attach after split/re-render): we simply move
 * the xterm root element into the new host, then re-fit. No new canvas,
 * no lost scrollback.
 */
export function attach(paneId: string, host: HTMLElement): void {
  const inst = instances.get(paneId);
  if (!inst) return;

  if (!inst.opened) {
    inst.term.open(host);
    inst.element = host.firstElementChild as HTMLElement ?? host;
    inst.opened = true;
    // Flush early chunks.
    for (const chunk of inst.earlyChunks) queueOutput(inst, chunk);
    inst.earlyChunks.length = 0;
  } else {
    // Move the existing xterm DOM subtree into the new host.
    // xterm renders into a div it appends to the element you pass to open().
    // That div is still alive — just reparent it.
    while (host.firstChild) host.removeChild(host.firstChild);
    // xterm's container is the first child of the original host.
    const xtermContainer = inst.term.element;
    if (xtermContainer && xtermContainer.parentElement !== host) {
      host.appendChild(xtermContainer);
    }
  }

  // Fit after attach so the terminal knows its new size immediately.
  try {
    inst.fitAddon.fit();
  } catch {
    /* host may not have layout yet; ResizeObserver will catch it */
  }
}

/**
 * Detach: the host element is going away (component destroy) but the instance
 * survives. We don't touch the xterm DOM — the element lives in `inst.element`
 * and will be re-attached to the next host.
 */
export function detach(_paneId: string): void {
  // Nothing to do right now — instance stays alive in the map.
  // If we needed to, we could reparent the element to a hidden off-screen
  // div here to prevent layout thrash, but in practice Svelte destroys+creates
  // fast enough that it's not necessary.
}

/** Write output to the instance (from pane:data events). */
export function writeToInstance(paneId: string, chunk: string): void {
  const inst = instances.get(paneId);
  if (!inst) return;
  if (!inst.opened) {
    inst.earlyChunks.push(chunk);
  } else {
    queueOutput(inst, chunk);
  }
}

/** Get xterm selection text. */
export function getSelection(paneId: string): string {
  return instances.get(paneId)?.term.getSelection() ?? "";
}

/** Paste text via bracketed paste. */
export function pasteToInstance(paneId: string, text: string): void {
  const inst = instances.get(paneId);
  if (!inst || !inst.opened) return;
  try {
    inst.term.paste(text);
  } catch {
    /* ignore */
  }
}

/** Focus the terminal. */
export function focusInstance(paneId: string): void {
  const inst = instances.get(paneId);
  if (inst?.opened) {
    try { inst.term.focus(); } catch { /* ignore */ }
  }
}

/** Resize the terminal's virtual viewport. */
export function fitInstance(paneId: string): boolean {
  const inst = instances.get(paneId);
  if (!inst || !inst.opened) return false;
  try {
    inst.fitAddon.fit();
    return true;
  } catch {
    return false;
  }
}

/** Get the current cols/rows of the terminal. */
export function getDimensions(paneId: string): { cols: number; rows: number } | null {
  const inst = instances.get(paneId);
  if (!inst) return null;
  return { cols: inst.term.cols, rows: inst.term.rows };
}

/**
 * Scroll the terminal viewport to the bottom (most recent line). Viewport-only
 * — does NOT send input to the PTY, so it's safe even when a TUI app (vim,
 * less, htop) is running. Used after a splitter drag so each pane snaps back to
 * its prompt instead of sitting at an arbitrary post-reflow scroll position.
 */
export function scrollInstanceToBottom(paneId: string): void {
  const inst = instances.get(paneId);
  if (!inst?.opened) return;
  try {
    inst.term.scrollToBottom();
  } catch {
    /* ignore */
  }
}

/** Fully destroy an instance (call when pane is closed, not just unmounted). */
export function destroyInstance(paneId: string): void {
  const inst = instances.get(paneId);
  if (!inst) return;
  instances.delete(paneId);
  if (inst.flushTimer) {
    window.clearTimeout(inst.flushTimer);
  }
  try {
    inst.term.dispose();
  } catch {
    /* ignore */
  }
}

export function hasInstance(paneId: string): boolean {
  return instances.has(paneId);
}
