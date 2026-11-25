<script>
  // Something that opens beside its button, and closes with Escape or a
  // click elsewhere. Fixed to the window, so no scrolling panel cuts it off.
  let { anchor, width = 240, height = 280, onclose, children } = $props();

  let panel;
  let place = $state({ left: 0, top: 0 });

  $effect(() => {
    const r = anchor.getBoundingClientRect();
    const left = Math.max(8, Math.min(r.left, innerWidth - width - 8));
    const below = r.bottom + 6;
    const top = below + height < innerHeight ? below : Math.max(8, r.top - height - 6);
    place = { left, top };
  });

  function outside(e) {
    if (!panel?.contains(e.target) && !anchor.contains(e.target)) onclose();
  }

  function key(e) {
    if (e.key !== 'Escape') return;
    e.stopPropagation();
    onclose();
    anchor.focus();
  }
</script>

<svelte:window onpointerdowncapture={outside} onkeydowncapture={key} onresize={() => onclose()} />

<div class="panel" bind:this={panel} style:left="{place.left}px" style:top="{place.top}px" style:width="{width}px" style:max-height="{height}px">
  {@render children()}
</div>

<style>
  .panel {
    position: fixed;
    z-index: 20;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    box-shadow: var(--shadow);
  }
</style>
