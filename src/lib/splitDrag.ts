/**
 * Tracks whether a splitter divider is currently being dragged.
 *
 * Why this exists: dragging a divider fires pointermove many times per
 * second, each one resizing the pane grid. Every pane's Terminal has a
 * ResizeObserver that responds by calling xterm's FitAddon, which triggers
 * `term.resize()`. xterm resize SYNCHRONOUSLY reflows the entire scrollback
 * buffer (re-wraps every stored line to the new column width) — that is
 * O(scrollback). Once scrollback fills up after extended use, each resize
 * takes long enough that doing it ~60×/s per pane pegs the main thread and
 * the whole window freezes.
 *
 * Fix: while a drag is in progress, Terminals skip fitting (the pane
 * containers still resize visually via the CSS grid — the xterm canvas just
 * stays at its old dimensions and clips, exactly like tmux/iTerm do mid-
 * drag). When the drag ends we notify subscribers so each Terminal runs a
 * single fit + PTY resize instead of one per frame.
 */

let dragging = false;
const endListeners = new Set<() => void>();

/** Called by Splitter when a divider drag begins. */
export function beginSplitDrag(): void {
  dragging = true;
}

/** Called by Splitter when the drag ends or is cancelled. Idempotent. */
export function endSplitDrag(): void {
  if (!dragging) return;
  dragging = false;
  // Copy to a stable iteration set so a listener that (un)subscribes during
  // dispatch can't perturb the loop.
  for (const fn of [...endListeners]) {
    try {
      fn();
    } catch {
      // One bad listener must not break the rest of the drag-end fan-out.
    }
  }
}

/** True while a divider drag is in progress. */
export function isSplitDragging(): boolean {
  return dragging;
}

/**
 * Register a callback fired once when the active drag ends.
 * Returns an unsubscribe function.
 */
export function onSplitDragEnd(fn: () => void): () => void {
  endListeners.add(fn);
  return () => {
    endListeners.delete(fn);
  };
}
