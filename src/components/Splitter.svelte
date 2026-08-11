<script lang="ts">
  import type { SplitDirection } from "../lib/PaneTree";
  import { beginSplitDrag, endSplitDrag } from "../lib/splitDrag";

  interface Props {
    direction: SplitDirection;
    containerEl: HTMLElement | undefined;
    onDrag: (ratio: number) => void;
  }

  let { direction, containerEl, onDrag }: Props = $props();

  let dragging = $state(false);

  function startDrag(ev: PointerEvent) {
    if (!containerEl) return;
    dragging = true;
    // Signal Terminals to skip their (expensive, scrollback-reflowing) fit
    // until we release — see splitDrag.ts. Commit one fit on drag end.
    beginSplitDrag();
    (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
    ev.preventDefault();
  }

  function move(ev: PointerEvent) {
    if (!dragging || !containerEl) return;
    const rect = containerEl.getBoundingClientRect();
    let ratio: number;
    if (direction === "h") {
      // horizontal divider → measure Y
      ratio = (ev.clientY - rect.top) / rect.height;
    } else {
      ratio = (ev.clientX - rect.left) / rect.width;
    }
    onDrag(ratio);
  }

  function endDrag(ev: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    (ev.currentTarget as HTMLElement).releasePointerCapture(ev.pointerId);
    // Releases the deferred-fit: each Terminal runs one fit + PTY resize now.
    endSplitDrag();
  }
</script>

<div
  class="splitter"
  class:horizontal={direction === "h"}
  class:vertical={direction === "v"}
  class:dragging
  onpointerdown={startDrag}
  onpointermove={move}
  onpointerup={endDrag}
  onpointercancel={endDrag}
  role="separator"
  aria-orientation={direction === "h" ? "horizontal" : "vertical"}
></div>

<style>
  .splitter {
    background: var(--border);
    transition: background 120ms ease;
    position: relative;
    z-index: 5;
  }
  .splitter.horizontal {
    width: 100%;
    height: var(--splitter-size);
    cursor: row-resize;
  }
  .splitter.vertical {
    height: 100%;
    width: var(--splitter-size);
    cursor: col-resize;
  }
  .splitter:hover,
  .splitter.dragging {
    background: var(--splitter-hover);
  }
</style>
