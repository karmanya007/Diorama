<script>
  import { app, scan, costOf, weight } from './state.svelte.js';
  import Preview from './Preview.svelte';

  const KINDS = [
    { id: 'all', label: 'All' },
    { id: 'scene', label: 'Scenes' },
    { id: 'video', label: 'Videos' },
    { id: 'web', label: 'Web' },
    { id: 'image', label: 'Images' },
  ];
  const SORTS = { name: 'Name', modified: 'Recently changed', size: 'Size on disk', light: 'Lightest first' };

  let query = $state('');
  let kind = $state('all');
  let tag = $state('');
  let source = $state('all');
  let sort = $state('name');
  let mature = $state(false);
  let hovered = $state(null);

  const playable = $derived(
    app.items.filter((i) => ['scene', 'video', 'web', 'image'].includes(i.kind)),
  );
  const tags = $derived(
    [...new Set(playable.flatMap((i) => i.tags))].sort((a, b) => a.localeCompare(b)),
  );
  // Questionable and mature items stay hidden unless asked for (P15).
  const hiddenRated = $derived(
    playable.filter((i) => i.rating === 'questionable' || i.rating === 'mature').length,
  );
  const shown = $derived.by(() => {
    const q = query.trim().toLowerCase();
    const list = playable.filter(
      (i) =>
        (kind === 'all' || i.kind === kind) &&
        (source === 'all' || i.source === source) &&
        (!tag || i.tags.includes(tag)) &&
        (mature || (i.rating !== 'questionable' && i.rating !== 'mature')) &&
        (!q ||
          i.title.toLowerCase().includes(q) ||
          i.tags.some((t) => t.toLowerCase().includes(q)) ||
          i.description.toLowerCase().includes(q) ||
          (i.workshop_id ?? '').includes(q)),
    );
    const by = {
      name: (a, b) => a.title.localeCompare(b.title),
      modified: (a, b) => b.modified - a.modified,
      size: (a, b) => b.size - a.size,
      // Measured ones by load, then the rest by name.
      light: (a, b) => {
        const [x, y] = [costOf(a), costOf(b)];
        if (x && y) return x.gpu + x.cpu - (y.gpu + y.cpu);
        return x ? -1 : y ? 1 : a.title.localeCompare(b.title);
      },
    }[sort];
    return list.sort(by);
  });

  const playing = $derived(
    new Set(
      (app.status?.outputs ?? [])
        .map((o) => o.content?.path?.replaceAll('\\', '/').toLowerCase())
        .filter(Boolean),
    ),
  );
  const isPlaying = (item) => {
    const p = item.path.replaceAll('\\', '/').toLowerCase();
    return [...playing].some((x) => x === p || x.startsWith(p + '/'));
  };
  const counts = $derived(
    Object.fromEntries(KINDS.map((k) => [k.id, playable.filter((i) => k.id === 'all' || i.kind === k.id).length])),
  );
</script>

<header>
  <div class="row top">
    <h1>Library</h1>
    <span class="muted">{shown.length} of {playable.length}</span>
    <div class="spacer"></div>
    <input type="search" placeholder="Search titles, tags, Workshop ids" bind:value={query} aria-label="Search" />
    <button onclick={scan} disabled={app.scanning} title="Look for new wallpapers">
      {app.scanning ? 'Scanning…' : 'Rescan'}
    </button>
  </div>
  <div class="row filters">
    <div class="segmented" role="group" aria-label="Kind">
      {#each KINDS as k (k.id)}
        <button class:on={kind === k.id} onclick={() => (kind = k.id)}>
          {k.label} <span class="count">{counts[k.id]}</span>
        </button>
      {/each}
    </div>
    <select bind:value={source} aria-label="Source">
      <option value="all">All sources</option>
      <option value="workshop">Workshop</option>
      <option value="wallpaper-engine">Wallpaper Engine</option>
      <option value="folder">My folders</option>
    </select>
    <select bind:value={tag} aria-label="Tag">
      <option value="">All tags</option>
      {#each tags as t (t)}
        <option value={t}>{t}</option>
      {/each}
    </select>
    <select bind:value={sort} aria-label="Sort">
      {#each Object.entries(SORTS) as [id, label] (id)}
        <option value={id}>{label}</option>
      {/each}
    </select>
    <label class="row toggle" title="Questionable and mature content stays hidden unless this is on">
      <input type="checkbox" bind:checked={mature} />
      Show mature{hiddenRated ? ` (${hiddenRated})` : ''}
    </label>
  </div>
</header>

<section class="grid scroll">
  {#if app.scanning && !app.items.length}
    <p class="muted pad">Looking for wallpapers…</p>
  {:else if !shown.length}
    <p class="muted pad">
      {playable.length ? 'Nothing matches these filters.' : 'No wallpapers found. Add folders in Settings, or subscribe to Workshop items in Wallpaper Engine.'}
    </p>
  {/if}
  {#each shown as item (item.path)}
    <div
      class="tile"
      role="button"
      tabindex="0"
      class:selected={app.selected?.path === item.path}
      onclick={() => (app.selected = item)}
      onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), (app.selected = item))}
      onmouseenter={() => (hovered = item.path)}
      onmouseleave={() => (hovered = null)}
      title={item.title}
    >
      <div class="thumb">
        <Preview path={item.preview} live={hovered === item.path} alt="" />
        <span class="kind">{item.kind}</span>
        {#if isPlaying(item)}<span class="live">Showing</span>{/if}
        {#if costOf(item)}
          {@const w = weight(costOf(item))}
          <span class="cost {w}" title="Measured while it played: {costOf(item).gpu.toFixed(0)}% GPU, {costOf(item).cpu.toFixed(0)}% CPU">{w}</span>
        {/if}
      </div>
      <div class="name">{item.title}</div>
    </div>
  {/each}
</section>

<style>
  header {
    padding: 18px 20px 10px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    border-bottom: 1px solid var(--line);
  }

  .top input {
    width: min(320px, 40vw);
  }

  .filters {
    flex-wrap: wrap;
  }

  .segmented {
    display: inline-flex;
    border: 1px solid var(--line);
    border-radius: 9px;
    overflow: hidden;
    background: var(--panel);
  }

  .segmented button {
    border: none;
    border-radius: 0;
    padding: 5px 12px;
    background: transparent;
  }

  .segmented button.on {
    background: var(--accent-soft);
    font-weight: 600;
  }

  .count {
    color: var(--muted);
    font-size: 12px;
    margin-left: 2px;
  }

  .toggle {
    gap: 6px;
    color: var(--muted);
  }

  .grid {
    flex: 1;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
    /* The grid scrolls itself: auto rows would shrink to fit its height. */
    grid-auto-rows: max-content;
    gap: 14px;
    align-content: start;
    padding: 16px 20px 24px;
  }

  .pad {
    grid-column: 1 / -1;
    padding: 24px 0;
  }

  .tile {
    display: flex;
    flex-direction: column;
    cursor: pointer;
    padding: 0;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--panel);
    text-align: left;
    box-shadow: var(--shadow);
    transition: transform 0.12s ease, border-color 0.12s ease;
  }

  .tile:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .tile:hover {
    transform: translateY(-2px);
  }

  .tile.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent);
  }

  .thumb {
    position: relative;
    width: 100%;
    aspect-ratio: 16 / 10;
    background: var(--panel-2);
  }

  .kind,
  .live {
    position: absolute;
    top: 8px;
    padding: 1px 7px;
    border-radius: 6px;
    font-size: 11px;
    font-weight: 600;
    text-transform: capitalize;
    background: rgba(10, 8, 20, 0.62);
    color: #fff;
  }

  .kind {
    left: 8px;
  }

  .cost {
    position: absolute;
    left: 8px;
    bottom: 8px;
    padding: 1px 7px;
    border-radius: 6px;
    font-size: 11px;
    font-weight: 600;
    text-transform: capitalize;
    background: rgba(10, 8, 20, 0.62);
    color: #fff;
  }

  .cost::before {
    content: '';
    display: inline-block;
    width: 7px;
    height: 7px;
    margin-right: 5px;
    border-radius: 50%;
    background: var(--ok);
  }

  .cost.moderate::before {
    background: var(--warn);
  }

  .cost.heavy::before {
    background: var(--danger);
  }

  .live {
    right: 8px;
    background: var(--accent);
  }

  .name {
    width: 100%;
    padding: 8px 10px 10px;
    font-weight: 500;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
