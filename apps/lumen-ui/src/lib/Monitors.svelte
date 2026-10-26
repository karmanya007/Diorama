<script>
  import { app, attempt, refreshStatus, itemFor, monitorName } from './state.svelte.js';
  import * as api from './api.js';
  import Preview from './Preview.svelte';

  let { onopen } = $props();

  const outputs = $derived(app.status?.outputs ?? []);
  // The desktop's bounding box, to place monitors as they are arranged.
  const box = $derived.by(() => {
    if (!outputs.length) return null;
    const x0 = Math.min(...outputs.map((o) => o.x));
    const y0 = Math.min(...outputs.map((o) => o.y));
    const x1 = Math.max(...outputs.map((o) => o.x + o.width));
    const y1 = Math.max(...outputs.map((o) => o.y + o.height));
    return { x0, y0, w: x1 - x0, h: y1 - y0 };
  });

  const pct = (v, total) => `${(v / total) * 100}%`;

  function choose(o) {
    app.target = [o.id];
    onopen();
  }
</script>

<header class="row">
  <h1>Monitors</h1>
  <span class="muted">Pick a monitor, then choose what it shows from the library.</span>
</header>

<section class="scroll">
  {#if box}
    <div
      class="desk"
      style:aspect-ratio="{box.w} / {box.h}"
      style:width="min(100%, 760px, {(46 * box.w) / box.h}vh)"
    >
      {#each outputs as o (o.id)}
        {@const item = itemFor(o.content?.path)}
        <button
          class="screen"
          class:paused={o.paused}
          style:left={pct(o.x - box.x0, box.w)}
          style:top={pct(o.y - box.y0, box.h)}
          style:width="calc({pct(o.width, box.w)} - 6px)"
          style:height="calc({pct(o.height, box.h)} - 6px)"
          onclick={() => choose(o)}
          title="Choose what {monitorName(o)} shows"
        >
          <Preview path={item?.preview ?? (item?.kind === 'image' ? item.path : null)} alt="" />
          <span class="label">
            <strong>{monitorName(o)}{o.primary ? ' ★' : ''}</strong>
            <span>{o.width}×{o.height} · {Math.round(o.refresh_hz)} Hz</span>
          </span>
        </button>
      {/each}
    </div>

    <div class="list">
      {#each outputs as o (o.id)}
        {@const item = itemFor(o.content?.path)}
        <div class="card entry">
          <div class="row">
            <h2>{monitorName(o)}</h2>
            {#if o.primary}<span class="chip">primary</span>{/if}
            <span class="chip">{o.paused ? `paused (${o.coverage})` : 'playing'}</span>
            <div class="spacer"></div>
            {#if o.playlist}
              <span class="chip">playlist: {o.playlist}</span>
              <button onclick={() => attempt(async () => { await api.next([o.id]); await refreshStatus(); })}>Next</button>
            {/if}
            <button class="primary" onclick={() => choose(o)}>Change…</button>
          </div>
          <div class="muted">
            {#if o.content}
              {item?.title ?? o.content.path} · {o.content.kind}
            {:else}
              Nothing (the Windows wallpaper shows)
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {:else}
    <p class="muted">Waiting for lumend…</p>
  {/if}
</section>

<style>
  header {
    padding: 18px 20px 12px;
    gap: 14px;
    border-bottom: 1px solid var(--line);
  }

  section {
    flex: 1;
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 22px;
  }

  .desk {
    position: relative;
    margin: 0 auto;
  }

  .screen {
    position: absolute;
    padding: 0;
    overflow: hidden;
    border: 2px solid var(--line);
    border-radius: 6px;
    background: var(--panel-2);
    margin: 3px;
  }

  .screen:hover {
    border-color: var(--accent);
  }

  .screen.paused :global(img) {
    filter: grayscale(0.8) brightness(0.7);
  }

  .label {
    position: absolute;
    left: 6px;
    bottom: 6px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    padding: 3px 8px;
    border-radius: 6px;
    font-size: 11px;
    background: rgba(10, 8, 20, 0.62);
    color: #fff;
  }

  .list {
    display: grid;
    gap: 10px;
  }

  .entry {
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
</style>
