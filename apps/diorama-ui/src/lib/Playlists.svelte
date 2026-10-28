<script>
  import { app, attempt, refreshStatus, toast, itemFor, monitorName } from './state.svelte.js';
  import * as api from './api.js';
  import Preview from './Preview.svelte';

  let lists = $state([]);
  let editing = $state(null);
  let original = $state('');
  let playOn = $state('');

  const load = () => attempt(async () => (lists = await api.playlists()));
  $effect(() => {
    load();
  });

  const playable = $derived(
    app.items
      .filter((i) => ['scene', 'video', 'web', 'image'].includes(i.kind))
      .sort((a, b) => a.title.localeCompare(b.title)),
  );
  const outputs = $derived(app.status?.outputs ?? []);
  const hhmm = (m) => `${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`;
  const minutes = (t) => {
    const [h, m] = t.split(':').map(Number);
    return (h * 60 + m) % 1440;
  };
  const title = (path) => itemFor(path)?.title ?? path.split(/[\\/]/).pop();
  const summary = (p) =>
    p.schedule.mode === 'interval'
      ? `every ${p.schedule.minutes} min${p.order === 'shuffle' ? ', shuffled' : ''}`
      : 'by time of day';
  const where = (p) =>
    outputs.filter((o) => o.playlist === p.name).map(monitorName).join(', ');

  function edit(p) {
    original = p?.name ?? '';
    editing = p
      ? {
          name: p.name,
          items: [...p.items],
          mode: p.schedule.mode,
          every: p.schedule.minutes ?? 30,
          shuffle: p.order === 'shuffle',
          times: p.schedule.starts?.map(hhmm) ?? p.items.map((_, i) => hhmm((420 + i * 120) % 1440)),
        }
      : { name: '', items: [], mode: 'interval', every: 30, shuffle: false, times: [] };
  }

  function add(path) {
    if (!path) return;
    const last = editing.times.at(-1);
    editing.items.push(path);
    editing.times.push(last ? hhmm((minutes(last) + 60) % 1440) : '07:00');
  }

  function remove(i) {
    editing.items.splice(i, 1);
    editing.times.splice(i, 1);
  }

  function move(i, d) {
    const j = i + d;
    if (j < 0 || j >= editing.items.length) return;
    [editing.items[i], editing.items[j]] = [editing.items[j], editing.items[i]];
    [editing.times[i], editing.times[j]] = [editing.times[j], editing.times[i]];
  }

  async function save() {
    const e = editing;
    const name = e.name.trim();
    if (!name || !e.items.length) {
      toast('A playlist needs a name and at least one wallpaper');
      return;
    }
    const playlist = {
      name,
      items: e.items,
      order: e.shuffle ? 'shuffle' : 'sequential',
      schedule:
        e.mode === 'interval'
          ? { mode: 'interval', minutes: Math.max(1, Math.round(Number(e.every) || 30)) }
          : { mode: 'time-of-day', starts: e.times.map(minutes) },
    };
    await attempt(async () => {
      await api.savePlaylist(playlist);
      if (original && original !== name) await api.deletePlaylist(original);
      editing = null;
      await load();
    });
  }

  async function remove_(p) {
    if (!confirm(`Delete the playlist ${p.name}?`)) return;
    await attempt(async () => {
      await api.deletePlaylist(p.name);
      await load();
      await refreshStatus();
    });
  }

  async function playIt(p) {
    await attempt(async () => {
      await api.play(p.name, playOn ? [playOn] : []);
      toast(`Playing ${p.name}`);
      await refreshStatus();
    });
  }
</script>

<header class="row">
  <h1>Playlists</h1>
  <div class="spacer"></div>
  <label class="row muted">
    Play on
    <select bind:value={playOn}>
      <option value="">all monitors</option>
      {#each outputs as o (o.id)}<option value={o.id}>{monitorName(o)}</option>{/each}
    </select>
  </label>
  <button class="primary" onclick={() => edit(null)}>New playlist</button>
</header>

<section class="scroll">
  {#if editing}
    <div class="card editor">
      <div class="row">
        <input type="text" placeholder="Playlist name" bind:value={editing.name} aria-label="Name" />
        <div class="spacer"></div>
        <button class="ghost" onclick={() => (editing = null)}>Cancel</button>
        <button class="primary" onclick={save}>Save</button>
      </div>
      <div class="row wrap">
        <label class="row"><input type="radio" bind:group={editing.mode} value="interval" /> Change every</label>
        <input type="number" min="1" class="minutes" bind:value={editing.every} disabled={editing.mode !== 'interval'} aria-label="Minutes" />
        <span class="muted">minutes</span>
        <label class="row"><input type="checkbox" bind:checked={editing.shuffle} disabled={editing.mode !== 'interval'} /> Shuffle</label>
        <span class="divider"></span>
        <label class="row"><input type="radio" bind:group={editing.mode} value="time-of-day" /> By time of day</label>
      </div>
      <ol class="items">
        {#each editing.items as path, i (path + i)}
          {@const item = itemFor(path)}
          <li class="row">
            <span class="thumb"><Preview path={item?.preview} alt="" /></span>
            {#if editing.mode === 'time-of-day'}
              <input type="time" bind:value={editing.times[i]} aria-label="Start time" />
            {/if}
            <span class="grow">{title(path)}</span>
            <button class="ghost" onclick={() => move(i, -1)} aria-label="Move up">↑</button>
            <button class="ghost" onclick={() => move(i, 1)} aria-label="Move down">↓</button>
            <button class="ghost danger" onclick={() => remove(i)} aria-label="Remove">✕</button>
          </li>
        {/each}
      </ol>
      <select onchange={(e) => { add(e.currentTarget.value); e.currentTarget.value = ''; }} aria-label="Add a wallpaper">
        <option value="">Add a wallpaper…</option>
        {#each playable as it (it.path)}<option value={it.path}>{it.title} ({it.kind})</option>{/each}
      </select>
    </div>
  {/if}

  {#if !lists.length && !editing}
    <p class="muted">No playlists yet. Create one here, or use “Add to playlist” on any wallpaper.</p>
  {/if}

  {#each lists as p (p.name)}
    <div class="card entry">
      <div class="row">
        <h2>{p.name}</h2>
        <span class="chip">{p.items.length} wallpapers · {summary(p)}</span>
        {#if where(p)}<span class="chip on">playing on {where(p)}</span>{/if}
        <div class="spacer"></div>
        <button class="ghost danger" onclick={() => remove_(p)}>Delete</button>
        <button onclick={() => edit(p)}>Edit</button>
        <button class="primary" onclick={() => playIt(p)}>Play</button>
      </div>
      <div class="strip">
        {#each p.items as path, i (path + i)}
          {@const item = itemFor(path)}
          <div class="mini" title={title(path)}>
            <Preview path={item?.preview} alt="" />
            {#if p.schedule.mode === 'time-of-day'}<span class="time">{hhmm(p.schedule.starts[i] ?? 0)}</span>{/if}
          </div>
        {/each}
      </div>
    </div>
  {/each}
</section>

<style>
  header {
    padding: 18px 20px 12px;
    gap: 12px;
    border-bottom: 1px solid var(--line);
  }

  section {
    flex: 1;
    padding: 18px 20px 24px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .editor,
  .entry {
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .editor input[type='text'] {
    width: 260px;
  }

  .wrap {
    flex-wrap: wrap;
  }

  .minutes {
    width: 72px;
  }

  .divider {
    width: 1px;
    height: 20px;
    background: var(--line);
    margin: 0 6px;
  }

  .items {
    margin: 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .thumb {
    position: relative;
    width: 64px;
    height: 40px;
    border-radius: 6px;
    overflow: hidden;
    flex: none;
  }

  .grow {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .strip {
    display: flex;
    gap: 8px;
    overflow-x: auto;
    padding-bottom: 4px;
  }

  .mini {
    position: relative;
    width: 120px;
    aspect-ratio: 16 / 10;
    flex: none;
    border-radius: 8px;
    overflow: hidden;
    background: var(--panel-2);
  }

  .time {
    position: absolute;
    left: 6px;
    bottom: 6px;
    padding: 0 6px;
    border-radius: 5px;
    font-size: 11px;
    background: rgba(10, 8, 20, 0.62);
    color: #fff;
  }

  .chip.on {
    background: var(--accent-soft);
    color: var(--text);
  }
</style>
