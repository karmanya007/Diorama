<script>
  // A wallpaper's properties as a form, built from its project.json. Values
  // come from lumend (defaults, then WE's choices, then Lumen's); every
  // change is sent at once, so a showing wallpaper updates live.
  import { attempt } from './state.svelte.js';
  import * as api from './api.js';
  import { fields, labelOf, toHex, fromHex, holds } from './props.js';

  let { item } = $props();

  let values = $state({});
  let loaded = $state(false);
  const list = $derived(fields(item.properties));

  $effect(() => {
    const path = item.path;
    loaded = false;
    attempt(async () => {
      values = await api.properties(path);
      loaded = true;
    });
  });

  const timers = {};
  function change(name, value, delay = 0) {
    values[name] = value;
    clearTimeout(timers[name]);
    timers[name] = setTimeout(
      () => attempt(() => api.setProperties(item.path, { [name]: value })),
      delay,
    );
  }

  async function resetAll() {
    const changes = Object.fromEntries([...list.map((f) => f.name), 'cameraparallax'].map((n) => [n, null]));
    await attempt(async () => {
      await api.setProperties(item.path, changes);
      values = await api.properties(item.path);
    });
  }

  const num = (v, d = 0) => (Number.isFinite(Number(v)) ? Number(v) : d);
  const step = (def) => def.step ?? (def.fraction ? 0.01 : 1);
</script>

{#if !loaded}
  <p class="muted">Reading values…</p>
{:else}
  <div class="form">
    {#if typeof values.cameraparallax === 'boolean'}
      <!-- Wallpaper Engine's per-wallpaper switch, for scenes with parallax. -->
      <label class="field check">
        <input type="checkbox" checked={values.cameraparallax} onchange={(e) => change('cameraparallax', e.currentTarget.checked)} />
        <span>Camera parallax (follows the mouse)</span>
      </label>
    {:else if !list.length}
      <p class="muted">No settings.</p>
    {/if}
    {#each list as f (f.name)}
      {#if holds(f.def.condition, values)}
        {#if f.type === 'text'}
          {@const label = labelOf(f.name, f.def)}
          {#if label}<div class="section">{label}</div>{/if}
        {:else if f.type === 'bool'}
          <label class="field check">
            <input type="checkbox" checked={!!values[f.name]} onchange={(e) => change(f.name, e.currentTarget.checked)} />
            <span>{labelOf(f.name, f.def)}</span>
          </label>
        {:else if f.type === 'slider'}
          <label class="field">
            <span class="row"><span>{labelOf(f.name, f.def)}</span><span class="spacer"></span><span class="muted">{num(values[f.name])}</span></span>
            <input
              type="range"
              min={num(f.def.min, 0)}
              max={num(f.def.max, 100)}
              step={step(f.def)}
              value={num(values[f.name])}
              oninput={(e) => change(f.name, Number(e.currentTarget.value), 200)}
            />
          </label>
        {:else if f.type === 'color'}
          <label class="field row">
            <span>{labelOf(f.name, f.def)}</span>
            <span class="spacer"></span>
            <input type="color" value={toHex(values[f.name])} oninput={(e) => change(f.name, fromHex(e.currentTarget.value), 150)} />
          </label>
        {:else if f.type === 'combo'}
          <label class="field">
            <span>{labelOf(f.name, f.def)}</span>
            <select
              value={String(values[f.name] ?? '')}
              onchange={(e) => {
                const opt = (f.def.options ?? []).find((o) => String(o.value) === e.currentTarget.value);
                change(f.name, opt ? opt.value : e.currentTarget.value);
              }}
            >
              {#each f.def.options ?? [] as o (String(o.value))}
                <option value={String(o.value)}>{labelOf('', { text: o.label }) || String(o.value)}</option>
              {/each}
            </select>
          </label>
        {:else if f.type === 'textinput'}
          <label class="field">
            <span>{labelOf(f.name, f.def)}</span>
            <input type="text" value={values[f.name] ?? ''} onchange={(e) => change(f.name, e.currentTarget.value)} />
          </label>
        {:else if f.type === 'file' || f.type === 'directory'}
          <label class="field">
            <span>{labelOf(f.name, f.def)} <span class="muted">({f.type === 'file' ? 'file path' : 'folder path'})</span></span>
            <span class="row">
              <input class="grow" type="text" placeholder="C:\…" value={values[f.name] ?? ''} onchange={(e) => change(f.name, e.currentTarget.value.trim())} />
              <button
                onclick={async (e) => {
                  e.preventDefault();
                  const path = await attempt(() =>
                    api.pick({ folder: f.type === 'directory', fileType: f.def.fileType, title: labelOf(f.name, f.def) }),
                  );
                  if (path) change(f.name, path.replaceAll('\\', '/'));
                }}>Browse…</button
              >
              {#if values[f.name]}
                <button class="ghost" title="Clear" onclick={(e) => { e.preventDefault(); change(f.name, ''); }}>✕</button>
              {/if}
            </span>
          </label>
        {/if}
      {/if}
    {/each}
    <div class="row">
      <div class="spacer"></div>
      <button class="ghost" onclick={resetAll} title="Undo changes made in Lumen (Wallpaper Engine's choices stay)">Reset Lumen changes</button>
    </div>
  </div>
{/if}

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .field.row {
    flex-direction: row;
  }

  .field.check {
    flex-direction: row;
    align-items: center;
    gap: 8px;
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  .section {
    margin-top: 6px;
    font-weight: 600;
    font-size: 13px;
    border-bottom: 1px solid var(--line);
    padding-bottom: 4px;
  }

  input[type='color'] {
    width: 44px;
    height: 28px;
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 2px;
    background: var(--panel);
  }
</style>
