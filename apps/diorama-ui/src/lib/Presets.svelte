<script>
  import { app, attempt, refreshStatus, toast, itemFor, monitorName } from './state.svelte.js';
  import * as api from './api.js';
  import Preview from './Preview.svelte';

  const NUMBERS = [1, 2, 3, 4, 5, 6, 7, 8, 9];
  const MODIFIERS = {
    'ctrl-alt': 'Ctrl+Alt',
    'ctrl-shift': 'Ctrl+Shift',
    'alt-shift': 'Alt+Shift',
    'ctrl-alt-shift': 'Ctrl+Alt+Shift',
  };

  let presets = $state([]);
  let name = $state('');

  const hotkeys = $derived(app.status?.hotkeys);
  const outputs = $derived(app.status?.outputs ?? []);
  const free = $derived(NUMBERS.find((n) => !presets.some((p) => p.number === n)));
  const showing = $derived(outputs.some((o) => o.content));
  const keys = (n) => `${MODIFIERS[hotkeys?.modifiers] ?? 'Ctrl+Alt'}+${n}`;
  const label = (p) => p.name || `Preset ${p.number}`;

  const load = () => attempt(async () => (presets = await api.monitorPresets()));
  $effect(() => {
    load();
  });

  const title = (w) =>
    w.playlist
      ? `playlist ${w.playlist.name}`
      : (itemFor(w.path)?.title ?? w.path.split(/[\\/]/).pop());

  function where(w) {
    if (!w.outputs?.length) return '';
    const names = w.outputs.map((id) => {
      const o = outputs.find((x) => x.id === id);
      return o ? monitorName(o) : 'a monitor not connected';
    });
    return ` on ${names.join(', ')}`;
  }

  const summary = (p) => p.wallpapers.map((w) => title(w) + where(w)).join(' · ');

  async function change(fn) {
    await attempt(async () => {
      await fn();
      await load();
      await refreshStatus();
    });
  }

  const add = () =>
    change(async () => {
      await api.savePreset(free, name.trim());
      name = '';
    });

  function update(p) {
    if (!confirm(`Replace ${label(p)} with what the monitors show now?`)) return;
    change(() => api.savePreset(p.number));
  }

  function remove(p) {
    if (!confirm(`Delete ${label(p)}?`)) return;
    change(() => api.deletePreset(p.number));
  }

  const show = (p) =>
    attempt(async () => {
      await api.usePreset(p.number);
      await refreshStatus();
      toast(`Showing ${label(p)}`);
    });
</script>

<div class="card block">
  <h2>Presets</h2>
  <p class="muted">Save what the monitors show now, and bring it back later with one click or a hotkey.</p>

  {#each presets as p (p.number)}
    {@const first = itemFor(p.wallpapers[0]?.path)}
    <div class="row preset">
      <span class="number">{p.number}</span>
      <span class="thumb"><Preview path={first?.preview ?? (first?.kind === 'image' ? first.path : null)} alt="" /></span>
      <div class="grow">
        <input
          type="text"
          value={p.name}
          maxlength="60"
          placeholder="Preset {p.number}"
          aria-label="Name of preset {p.number}"
          onchange={(e) => change(() => api.renamePreset(p.number, e.currentTarget.value))}
        />
        <div class="muted what" title={summary(p)}>{summary(p)}</div>
      </div>
      {#if hotkeys?.enabled && hotkeys.supported}
        {#if hotkeys.taken?.includes(p.number)}
          <span class="chip warn" title="Another program has this hotkey">{keys(p.number)} is taken</span>
        {:else}
          <span class="chip">{keys(p.number)}</span>
        {/if}
      {/if}
      <button class="primary" onclick={() => show(p)}>Show</button>
      <button onclick={() => update(p)} disabled={!showing} title="Replace it with what the monitors show now">Update</button>
      <button class="ghost danger" onclick={() => remove(p)}>Delete</button>
    </div>
  {/each}

  {#if free}
    <div class="row">
      <span class="number">{free}</span>
      <input
        class="grow"
        type="text"
        maxlength="60"
        placeholder="Name (optional)"
        aria-label="Name of the new preset"
        bind:value={name}
        onkeydown={(e) => e.key === 'Enter' && showing && add()}
      />
      <button onclick={add} disabled={!showing}>Save what the monitors show</button>
    </div>
  {/if}

  {#if hotkeys?.supported}
    <div class="row wrap">
      <label class="row">
        <input
          type="checkbox"
          checked={hotkeys.enabled}
          onchange={(e) => change(() => api.setHotkeys({ enabled: e.currentTarget.checked }))}
        />
        Hotkeys:
      </label>
      <select
        value={hotkeys.modifiers}
        disabled={!hotkeys.enabled}
        aria-label="Keys held with the number"
        onchange={(e) => change(() => api.setHotkeys({ modifiers: e.currentTarget.value }))}
      >
        {#each Object.entries(MODIFIERS) as [value, text] (value)}<option {value}>{text}</option>{/each}
      </select>
      <span>+ the preset's number, in any program</span>
    </div>
    {#if hotkeys.enabled && hotkeys.modifiers === 'ctrl-alt'}
      <p class="muted">Where AltGr types symbols on the digit keys, it counts as Ctrl+Alt: choose other keys if one stops working.</p>
    {/if}
  {:else if hotkeys}
    <p class="muted">
      This desktop does not let programs set global hotkeys. Bind keys to <code>diorama preset 1</code> and so on in its keyboard settings.
    </p>
  {/if}
</div>

<style>
  .block {
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .block p {
    margin: 0;
  }

  .number {
    width: 22px;
    text-align: center;
    font-weight: 600;
    color: var(--muted);
  }

  .thumb {
    width: 64px;
    height: 40px;
    position: relative;
    flex: none;
    border-radius: 6px;
    overflow: hidden;
    background: var(--panel-2);
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  .grow input {
    width: 100%;
    max-width: 320px;
  }

  .what {
    font-size: 12px;
    margin-top: 2px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .wrap {
    flex-wrap: wrap;
  }

  .chip.warn {
    color: var(--warn);
  }

  code {
    user-select: text;
  }
</style>
