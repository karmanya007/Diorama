<script>
  // The settings viewers of the wallpaper get: names, order, defaults.
  import { fromHex, labelOf } from '../props.js';
  import Field from './Field.svelte';

  let { editor } = $props();

  const KINDS = { slider: 'number', color: 'color', bool: 'flag', combo: 'choice', textinput: 'text' };
  const num = (v, d) => (Number.isFinite(Number(v)) ? Number(v) : d);
  const options = (def) => (def.options ?? []).map((o) => [String(o.value), String(o.label ?? o.value)]);

  function set(s, v) {
    const value = s.type === 'color' && /^#/.test(v ?? '') ? fromHex(v) : v;
    editor.changeSetting(s.name, (def) => (def.value = value), 'setting');
  }
</script>

<section>
  <h3>Viewers can change</h3>
  {#each editor.settings as s, i (s.name)}
    <div class="setting">
      <div class="row">
        <input
          class="grow"
          type="text"
          maxlength="60"
          aria-label="Name of the setting"
          value={labelOf(s.name, s.def)}
          onchange={(e) => editor.changeSetting(s.name, (def) => (def.text = e.currentTarget.value.trim() || s.name))}
        />
        <button class="ghost small" onclick={() => editor.moveSetting(s.name, -1)} disabled={i === 0} aria-label="Earlier">↑</button>
        <button class="ghost small" onclick={() => editor.moveSetting(s.name, 1)} disabled={i === editor.settings.length - 1} aria-label="Later">↓</button>
        <button class="ghost small danger" onclick={() => editor.removeSetting(s.name)} aria-label="Remove" title="Takes it out; what it is used for keeps its value">✕</button>
      </div>
      {#if KINDS[s.type]}
        <Field
          label="Default"
          kind={KINDS[s.type]}
          value={s.type === 'color' ? editor.setting(s.name) : s.def.value}
          min={num(s.def.min, 0)}
          max={num(s.def.max, 1)}
          step={num(s.def.step, 0.01)}
          choices={options(s.def)}
          onchange={(v) => set(s, v)}
          ondone={() => editor.settle()}
        />
      {/if}
      {#if s.type === 'slider'}
        <div class="range">
          <span class="muted">From</span>
          <input type="number" value={num(s.def.min, 0)} step="any" aria-label="Least" onchange={(e) => editor.changeSetting(s.name, (def) => (def.min = Math.min(num(e.currentTarget.value, 0), num(def.max, 1))))} />
          <span class="muted">to</span>
          <input type="number" value={num(s.def.max, 1)} step="any" aria-label="Most" onchange={(e) => editor.changeSetting(s.name, (def) => (def.max = Math.max(num(e.currentTarget.value, 1), num(def.min, 0))))} />
        </div>
      {/if}
    </div>
  {:else}
    <p class="note">Nothing yet: use the button at the end of a value’s row.</p>
  {/each}
</section>

<style>
  section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .setting {
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  .small {
    padding: 0 6px;
  }

  .range {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    justify-content: flex-end;
  }

  .range input {
    width: 62px;
    padding: 3px 6px;
  }

  .note {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
  }
</style>
