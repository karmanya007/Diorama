<script>
  import { app, attempt, refreshStatus, toast, monitorName, costOf, weight, itemFor } from './state.svelte.js';
  import * as api from './api.js';
  import Preview from './Preview.svelte';
  import Properties from './Properties.svelte';

  const item = $derived(app.selected);
  const cost = $derived(item ? costOf(item) : null);
  const mine = (path) => item && itemFor(path)?.path === item.path;
  // How the item plays where it shows, and its conversion (P4).
  const playing = $derived(outputs.map((o) => o.content).find((c) => c && mine(c.path)) ?? null);
  const conversion = $derived((app.status?.conversions ?? []).find((c) => mine(c.path)) ?? null);

  async function convert() {
    await attempt(async () => {
      await api.convert(item.path);
      await refreshStatus();
    });
  }
  const outputs = $derived(app.status?.outputs ?? []);
  let fit = $state('');
  let layout = $state('per-output');
  let busy = $state(false);
  let lists = $state([]);
  let addTo = $state('');

  $effect(() => {
    // Playlists for "Add to playlist", fetched when another item opens.
    item;
    attempt(async () => (lists = await api.playlists()));
  });

  const size = (b) =>
    b > 1e9 ? `${(b / 1e9).toFixed(1)} GB` : b > 1e6 ? `${(b / 1e6).toFixed(1)} MB` : `${Math.ceil(b / 1e3)} KB`;
  // Workshop descriptions use BBCode.
  const plain = (s) => s.replace(/\[\/?[a-z*]+(?:=[^\]]*)?\]/gi, '').trim();

  function toggle(id) {
    app.target = app.target.includes(id) ? app.target.filter((x) => x !== id) : [...app.target, id];
  }

  async function show() {
    busy = true;
    await attempt(async () => {
      const span = layout === 'span' && (app.target.length === 0 || app.target.length > 1);
      await api.setWallpaper(item.path, app.target, fit || undefined, span ? 'span' : 'per-output');
      toast(`Showing ${item.title}`);
      await refreshStatus();
    });
    busy = false;
  }

  async function addToPlaylist() {
    const name = addTo === '+' ? prompt('New playlist name')?.trim() : addTo;
    addTo = '';
    if (!name) return;
    await attempt(async () => {
      const existing = lists.find((p) => p.name === name);
      const playlist = existing
        ? { ...existing, items: [...existing.items, item.path] }
        : { name, items: [item.path], order: 'sequential', schedule: { mode: 'interval', minutes: 30 } };
      if (playlist.schedule.mode === 'time-of-day') {
        // A new time-of-day entry starts an hour after the last one.
        const last = playlist.schedule.starts.at(-1) ?? 420;
        playlist.schedule = { ...playlist.schedule, starts: [...playlist.schedule.starts, (last + 60) % 1440] };
      }
      await api.savePlaylist(playlist);
      lists = await api.playlists();
      toast(`Added to ${name}`);
    });
  }
</script>

<aside class="scroll">
  <div class="hero">
    <Preview path={item.preview} live alt={item.title} />
    <button class="close ghost" onclick={() => (app.selected = null)} aria-label="Close">✕</button>
  </div>
  <div class="body">
    <h2>{item.title}</h2>
    <div class="row wrap">
      <span class="chip">{item.kind}</span>
      {#if item.rating !== 'unknown'}<span class="chip">{item.rating}</span>{/if}
      <span class="chip">{size(item.size)}</span>
      <span class="chip">{item.source === 'workshop' ? `Workshop ${item.workshop_id ?? ''}` : item.source === 'folder' ? 'My folder' : 'Wallpaper Engine'}</span>
    </div>
    {#if item.tags.length}
      <div class="row wrap">
        {#each item.tags as t (t)}<span class="chip tag">{t}</span>{/each}
      </div>
    {/if}
    {#if item.description}
      <p class="desc">{plain(item.description)}</p>
    {/if}
    {#if conversion && !conversion.error}
      <div class="notice">
        <span>Converting for hardware decoding… {Math.round(conversion.progress * 100)}%</span>
        <progress max="1" value={conversion.progress}></progress>
      </div>
    {:else if conversion?.error}
      <div class="notice">
        <span>Converting failed: {conversion.error}</span>
        <button onclick={convert}>Try again</button>
      </div>
    {:else if playing?.converted}
      <p class="muted small">Plays a copy converted for hardware decoding.</p>
    {:else if playing?.software_decode}
      <div class="notice">
        <span>Your graphics card cannot decode this video, so the processor does (costly). Convert it to a copy the graphics card decodes, sized for your monitors?</span>
        <button onclick={convert}>Convert</button>
      </div>
    {/if}
    {#if cost}
      <p class="cost">
        <span class="dot {weight(cost)}"></span>
        <span>
          <strong>{weight(cost)[0].toUpperCase() + weight(cost).slice(1)}</strong>:
          {cost.gpu.toFixed(1)}% GPU, {cost.cpu.toFixed(1)}% CPU, {cost.vram_mb} MB video memory,
          {cost.ram_mb} MB memory
          <span class="muted">
            (measured {Math.max(1, Math.round(cost.seconds / 60))} min on {(cost.pixels / 1e6).toFixed(1)} megapixels{cost.fps
              ? ` at ${cost.fps} fps`
              : ''})
          </span>
        </span>
      </p>
    {/if}

    <h3>Show on</h3>
    <div class="row wrap">
      <button class="pill" class:on={app.target.length === 0} onclick={() => (app.target = [])}>All monitors</button>
      {#each outputs as o (o.id)}
        <button class="pill" class:on={app.target.includes(o.id)} onclick={() => toggle(o.id)}>
          {monitorName(o)}{o.primary ? ' ★' : ''}
        </button>
      {/each}
    </div>
    {#if outputs.length > 1 && app.target.length !== 1}
      <div class="segmented" role="group" aria-label="Layout">
        <button class:on={layout === 'per-output'} onclick={() => (layout = 'per-output')}>Whole picture on each</button>
        <button class:on={layout === 'span'} onclick={() => (layout = 'span')}>Span across them</button>
      </div>
    {/if}
    <div class="row">
      <select bind:value={fit} aria-label="Fit">
        <option value="">Fit: as before</option>
        <option value="fill">Fill (crop)</option>
        <option value="fit">Fit (bars)</option>
        <option value="stretch">Stretch</option>
      </select>
      <div class="spacer"></div>
      <button class="primary" onclick={show} disabled={busy || !app.status}>
        {busy ? 'Loading…' : 'Show'}
      </button>
    </div>
    <div class="row">
      <select bind:value={addTo} onchange={addToPlaylist} aria-label="Add to playlist">
        <option value="">Add to playlist…</option>
        {#each lists as p (p.name)}<option value={p.name}>{p.name}</option>{/each}
        <option value="+">New playlist…</option>
      </select>
    </div>

    {#if item.properties && Object.keys(item.properties).length}
      <h3>Properties</h3>
      <Properties {item} />
    {/if}
  </div>
</aside>

<style>
  aside {
    width: 380px;
    border-left: 1px solid var(--line);
    background: var(--panel);
    display: flex;
    flex-direction: column;
  }

  .hero {
    position: relative;
    aspect-ratio: 16 / 10;
    flex: none;
    background: var(--panel-2);
  }

  .close {
    position: absolute;
    top: 8px;
    right: 8px;
    background: rgba(10, 8, 20, 0.55);
    color: #fff;
    border-radius: 50%;
    width: 30px;
    height: 30px;
    padding: 0;
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px 18px 24px;
  }

  .body h3 {
    margin-top: 6px;
  }

  .wrap {
    flex-wrap: wrap;
  }

  .tag {
    background: var(--accent-soft);
    color: var(--text);
  }

  .desc {
    margin: 0;
    color: var(--muted);
    white-space: pre-line;
    max-height: 8.5em;
    overflow: auto;
    user-select: text;
  }

  .pill {
    border-radius: 999px;
    padding: 4px 12px;
  }

  .segmented {
    display: flex;
    border: 1px solid var(--line);
    border-radius: 9px;
    overflow: hidden;
  }

  .segmented button {
    flex: 1;
    border: none;
    border-radius: 0;
    background: transparent;
    padding: 6px 8px;
  }

  .segmented button.on {
    background: var(--accent-soft);
    font-weight: 600;
  }

  .pill.on {
    background: var(--accent-soft);
    border-color: var(--accent);
    font-weight: 600;
  }

  select {
    min-width: 0;
  }

  .cost {
    display: flex;
    gap: 8px;
    align-items: baseline;
    margin: 0;
    font-size: 13px;
  }

  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ok);
  }

  .dot.moderate {
    background: var(--warn);
  }

  .dot.heavy {
    background: var(--danger);
  }

  .notice {
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-start;
    padding: 10px 12px;
    border-radius: var(--radius);
    border: 1px solid var(--warn);
    font-size: 13px;
  }

  .notice progress {
    width: 100%;
    accent-color: var(--accent);
  }

  .small {
    margin: 0;
    font-size: 12px;
  }
</style>
