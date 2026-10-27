<script>
  import { app, attempt, refreshStatus, scan, toast } from './state.svelte.js';
  import * as api from './api.js';

  const DESCRIBE = {
    balanced: 'Pauses behind fullscreen apps and maximized windows, and while locked. 30 fps.',
    quality: 'Keeps playing beside maximized windows; smoother scenes at 60 fps.',
    gaming: 'Leaves the GPU to games, also pauses on battery. 24 fps.',
    battery: 'Pauses whenever it can and runs scenes slowly. 15 fps.',
  };
  const RULES = [
    ['fullscreen', 'A fullscreen app covers a monitor'],
    ['maximized', 'A maximized window covers a monitor'],
    ['battery', 'Running on battery'],
    ['locked', 'The PC is locked'],
  ];

  let presets = $state([]);
  let auto = $state(null);
  let newFolder = $state('');
  let volume = $state(0);
  let version = $state('');
  let update = $state(null);
  let checking = $state(false);
  let checked = $state('');
  let installing = $state(null);

  const policy = $derived(app.status?.policy);
  const current = $derived(
    policy ? (presets.find(([, p]) => JSON.stringify(p) === JSON.stringify(policy))?.[0] ?? 'custom') : '',
  );
  const folders = $derived(app.status?.folders ?? []);

  $effect(() => {
    attempt(async () => (presets = await api.presets()));
    attempt(async () => (auto = await api.autostart()));
    attempt(async () => (version = await api.version()));
  });

  async function checkUpdates() {
    checking = true;
    await attempt(async () => {
      update = await api.checkUpdate();
      checked = update ? '' : `Lumen ${version} is the latest version.`;
    });
    checking = false;
  }

  async function installUpdate() {
    installing = 0;
    await attempt(() => api.installUpdate(update, (p) => (installing = p)));
    installing = null;
  }
  $effect(() => {
    if (app.status) volume = app.status.volume;
  });

  async function apply(next) {
    await attempt(async () => {
      await api.setPolicy(next);
      await refreshStatus();
    });
  }

  let volumeTimer;
  function changeVolume(v) {
    volume = v;
    clearTimeout(volumeTimer);
    volumeTimer = setTimeout(() => attempt(() => api.setVolume(v)), 200);
  }

  async function setFolders(list) {
    await attempt(async () => {
      await api.setFolders(list);
      await refreshStatus();
      await scan();
    });
  }

  function addFolder() {
    const f = newFolder.trim();
    if (!f) return;
    if (folders.includes(f)) {
      toast('That folder is already in the library');
      return;
    }
    newFolder = '';
    setFolders([...folders, f]);
  }
</script>

<header><h1>Settings</h1></header>

<section class="scroll">
  <div class="card block">
    <h2>When to pause</h2>
    <div class="presets">
      {#each presets as [name, p] (name)}
        <button class="preset" class:on={current === name} onclick={() => apply(p)}>
          <strong>{name}</strong>
          <span class="muted">{DESCRIBE[name] ?? ''}</span>
        </button>
      {/each}
    </div>
    {#if policy}
      <h3>{current === 'custom' ? 'Custom rules' : 'Rules'}</h3>
      <div class="rules">
        {#each RULES as [key, label] (key)}
          <label class="row">
            <span class="grow">{label}</span>
            <select value={policy[key]} onchange={(e) => apply({ ...policy, [key]: e.currentTarget.value })}>
              <option value="pause">Pause</option>
              <option value="keep">Keep playing</option>
            </select>
          </label>
        {/each}
        <label class="row">
          <span class="grow">Scene frame rate cap</span>
          <input type="range" min="10" max="144" step="1" value={policy.fps} onchange={(e) => apply({ ...policy, fps: Number(e.currentTarget.value) })} />
          <span class="value">{policy.fps} fps</span>
        </label>
      </div>
    {/if}
  </div>

  <div class="card block">
    <h2>Sound</h2>
    <label class="row">
      <span class="grow">Video and scene sound volume <span class="muted">(0 keeps the audio device closed)</span></span>
      <input type="range" min="0" max="100" value={volume} oninput={(e) => changeVolume(Number(e.currentTarget.value))} />
      <span class="value">{volume}%</span>
    </label>
  </div>

  <div class="card block">
    <h2>Library folders</h2>
    <p class="muted">Wallpaper Engine's Workshop items and projects are found through Steam. Add folders of your own wallpapers, images and videos:</p>
    {#each folders as f (f)}
      <div class="row folder">
        <code class="grow">{f}</code>
        <button class="ghost danger" onclick={() => setFolders(folders.filter((x) => x !== f))}>Remove</button>
      </div>
    {/each}
    <div class="row">
      <input class="grow" type="text" placeholder="C:\Users\you\Pictures\Wallpapers" bind:value={newFolder} onkeydown={(e) => e.key === 'Enter' && addFolder()} aria-label="Folder path" />
      <button onclick={addFolder} disabled={!newFolder.trim()}>Add</button>
      <button
        onclick={async () => {
          const path = await attempt(() => api.pick({ folder: true, title: 'Add a folder to the library' }));
          if (path) {
            newFolder = path;
            addFolder();
          }
        }}>Browse…</button
      >
    </div>
  </div>

  <div class="card block">
    <h2>Start-up</h2>
    <label class="row">
      <input
        type="checkbox"
        checked={!!auto}
        disabled={auto === null}
        onchange={(e) => attempt(async () => { await api.setAutostart(e.currentTarget.checked); auto = await api.autostart(); })}
      />
      Start Lumen when I sign in (wallpapers only; this window opens when you ask for it)
    </label>
  </div>

  <div class="card block">
    <h2>Updates</h2>
    {#if update}
      <p>Lumen {update.version} is available (you have {version}).</p>
      {#if update.body}<p class="muted notes">{update.body}</p>{/if}
      <div class="row">
        <button onclick={installUpdate} disabled={installing !== null}>
          {installing === null ? 'Install and restart' : `Installing… ${Math.round(installing * 100)}%`}
        </button>
      </div>
    {:else}
      <div class="row">
        <button onclick={checkUpdates} disabled={checking}>{checking ? 'Checking…' : 'Check for updates'}</button>
        <span class="muted">{checked || (version ? `Version ${version}` : '')}</span>
      </div>
    {/if}
  </div>

  <p class="muted about">
    Lumen {app.status ? `· daemon v${app.status.daemon_version} (pid ${app.status.pid})` : ''} · plays Wallpaper Engine content you already have; nothing is downloaded unless you check for updates.
  </p>
</section>

<style>
  header {
    padding: 18px 20px 12px;
    border-bottom: 1px solid var(--line);
  }

  section {
    flex: 1;
    padding: 18px 20px 24px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  section > :global(*) {
    width: 100%;
    max-width: 860px;
  }

  .block {
    padding: 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .block p {
    margin: 0;
  }

  .presets {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
    gap: 10px;
  }

  .preset {
    display: flex;
    flex-direction: column;
    gap: 4px;
    text-align: left;
    padding: 10px 12px;
  }

  .preset strong {
    text-transform: capitalize;
  }

  .preset.on {
    border-color: var(--accent);
    background: var(--accent-soft);
  }

  .rules {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .grow {
    flex: 1;
  }

  .value {
    width: 56px;
    text-align: right;
    color: var(--muted);
  }

  .folder code {
    font-size: 12px;
    user-select: text;
  }

  .about {
    font-size: 12px;
  }

  .notes {
    margin: 0;
    font-size: 12px;
    white-space: pre-line;
  }
</style>
