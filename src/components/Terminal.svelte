<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { resizePane } from "../lib/ipc";
  import { isSplitDragging, onSplitDragEnd } from "../lib/splitDrag";
  import {
    attach,
    detach,
    fitInstance,
    getDimensions,
    scrollInstanceToBottom,
  } from "../lib/terminalInstances";

  /**
   * Thin view component. Does NOT own the xterm instance — it borrows it from
   * terminalInstances. On mount it calls attach(), which either calls
   * term.open() for the first time or reparents the existing xterm DOM element
   * so scrollback survives splits and re-renders.
   */

  interface Props {
    paneId: string;
  }

  let { paneId }: Props = $props();

  let hostEl: HTMLDivElement | undefined = $state();
  let resizeObserver: ResizeObserver | null = null;
  let resizeRaf = 0;
  let disposed = false;
  /**
   * Set when a fit is triggered by a splitter drag release. After the fit
   * (which reflows scrollback) we scroll the viewport back to the bottom so
   * the pane shows its latest output/prompt instead of an arbitrary post-
   * reflow scroll position.
   */
  let scrollAfterNextFit = false;

  function scheduleFit(opts?: { scrollToEnd?: boolean }) {
    if (disposed) return;
    // Defer while a splitter divider is being dragged. term.resize() reflows
    // the whole scrollback (O(lines)), so running it per-pointermove frame
    // freezes the app once scrollback fills up. We run a single fit on drag
    // end instead — see splitDrag.ts.
    if (isSplitDragging()) return;
    if (opts?.scrollToEnd) scrollAfterNextFit = true;
    if (resizeRaf) cancelAnimationFrame(resizeRaf);
    resizeRaf = requestAnimationFrame(() => {
      resizeRaf = 0;
      if (disposed) return;
      // A drag may have started between scheduling and the frame firing.
      if (isSplitDragging()) return;
      const fitted = fitInstance(paneId);
      if (!fitted) {
        scrollAfterNextFit = false;
        return;
      }
      if (scrollAfterNextFit) {
        scrollAfterNextFit = false;
        scrollInstanceToBottom(paneId);
      }
      const dims = getDimensions(paneId);
      if (dims && dims.cols > 0 && dims.rows > 0) {
        void resizePane(paneId, dims.cols, dims.rows).catch(() => {
          // pane may have exited; ignore
        });
      }
    });
  }

  let unsubSplitDragEnd: (() => void) | null = null;

  onMount(() => {
    if (!hostEl) return;

    // Attach (or re-attach) the persistent xterm instance to this host div.
    // On first mount this calls term.open(); on re-mounts (after a split
    // causes a re-render) it just moves the existing DOM element here.
    attach(paneId, hostEl);

    resizeObserver = new ResizeObserver(() => {
      if (!disposed) scheduleFit();
    });
    resizeObserver.observe(hostEl);
    scheduleFit();

    // When a splitter drag ends, run the single deferred fit we suppressed
    // during the drag, then snap each pane back to its latest output.
    unsubSplitDragEnd = onSplitDragEnd(() => scheduleFit({ scrollToEnd: true }));
  });

  onDestroy(() => {
    disposed = true;
    if (resizeRaf) cancelAnimationFrame(resizeRaf);
    resizeRaf = 0;
    try { resizeObserver?.disconnect(); } catch {}
    resizeObserver = null;
    unsubSplitDragEnd?.();
    unsubSplitDragEnd = null;
    // Detach from this host, but keep the xterm instance alive.
    detach(paneId);
  });
</script>

<div class="term-host" bind:this={hostEl}></div>

<style>
  .term-host {
    width: 100%;
    height: 100%;
    background: #0e0e10;
    overflow: hidden;
  }
  :global(.term-host .xterm) {
    width: 100%;
    height: 100%;
    padding: 4px 6px;
  }
  :global(.term-host .xterm-viewport) {
    background-color: transparent !important;
  }
</style>
