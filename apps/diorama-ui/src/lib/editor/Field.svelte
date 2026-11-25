<script>
  // One value of the document: a number (slider and box), two numbers, a
  // colour, a choice, a switch, text or an image. With `link` (see
  // Source.svelte) it may move or come from a setting; what is shown and
  // changed is then its key, its value in silence or the setting's default.
  import ColorPicker from '../ColorPicker.svelte';
  import { labelOf, toHex } from '../props.js';
  import Field from './Field.svelte';
  import { RANGES, around, keyAt, sourceOf } from './motion.js';
  import Source from './Source.svelte';

  let { label, value, editor = null, link = null, kind = 'number', min = 0, max = 1, step = 0.01, unit = '', choices = [], onchange, ondone = () => {}, onpick = () => {}, slider = true, limit = [min, max] } = $props();

  const source = $derived(link ? sourceOf(link.raw) : null);
  const keyed = $derived(source === 'keys' ? keyAt(link.raw, editor.time) : -1);
  const [before, after] = $derived(source === 'keys' ? around(link.raw, editor.time) : [null, null]);
  const copy = (v) => JSON.parse(JSON.stringify(v ?? null));

  /** Adds a key with the value as it is here, or removes the one here. */
  function toggleKey() {
    editor.pause();
    const i = keyAt(link.raw, editor.time);
    if (i < 0) return onchange(copy(value));
    if (link.raw.keys.length === 1) return link.set(copy(value));
    editor.key = null;
    link.set({ ...copy(link.raw), keys: copy(link.raw.keys).filter((_, j) => j !== i) });
  }

  // Of a value that follows the sound.
  const heard = (key, v) => link.set({ ...copy(link.raw), [key]: v }, key);

  const digits = $derived(Math.max(0, Math.ceil(-Math.log10(step || 1))));
  const shown = $derived(Number.isFinite(Number(value)) ? Number(Number(value).toFixed(digits)) : '');
  const hex = $derived.by(() => {
    const text = String(value ?? '');
    if (/^#[0-9a-f]{6}/i.test(text)) return text.slice(0, 7);
    if (/^#[0-9a-f]{3}$/i.test(text)) return `#${[...text.slice(1)].map((c) => c + c).join('')}`;
    return toHex(Array.isArray(value) ? value.join(' ') : text);
  });

  function typed(text) {
    const n = Number(text);
    if (text.trim() !== '' && Number.isFinite(n)) onchange(Math.min(limit[1], Math.max(limit[0], n)));
    ondone();
  }
</script>

{#if kind !== 'none'}
<div class="field">
  <span class="label" title={label}>{label}</span>
  {#if kind === 'number'}
    {#if slider}
      <input class="grow" type="range" {min} {max} {step} value={shown} aria-label={label} oninput={(e) => onchange(Number(e.currentTarget.value))} onchange={ondone} />
    {/if}
    <input class="number" class:wide={!slider} type="number" min={limit[0]} max={limit[1]} {step} value={shown} aria-label={label} onchange={(e) => typed(e.currentTarget.value)} />
    {#if unit}<span class="unit">{unit}</span>{/if}
  {:else if kind === 'pair'}
    {#each [0, 1] as i (i)}
      <input
        class="number wide pair"
        type="number"
        {min}
        {max}
        {step}
        value={Math.round((value?.[i] ?? 0) * 100) / 100}
        aria-label="{label} {'XY'[i]}"
        title={'XY'[i]}
        onchange={(e) => {
          const n = Number(e.currentTarget.value);
          if (e.currentTarget.value.trim() !== '' && Number.isFinite(n)) onchange(value.map((v, j) => (j === i ? Math.min(max, Math.max(min, n)) : v)));
          ondone();
        }}
      />
    {/each}
    {#if unit}<span class="unit">{unit}</span>{/if}
  {:else if kind === 'text'}
    <input class="grow" type="text" value={value ?? ''} aria-label={label} onchange={(e) => { onchange(e.currentTarget.value); ondone(); }} />
  {:else if kind === 'color'}
    <ColorPicker value={hex} {label} {onchange} {ondone} />
    <code class="muted">{hex}</code>
  {:else if kind === 'choice'}
    <select class="grow" {value} aria-label={label} onchange={(e) => { onchange(e.currentTarget.value); ondone(); }}>
      {#each choices as [id, text] (id)}<option value={id}>{text}</option>{/each}
    </select>
  {:else if kind === 'flag'}
    <input type="checkbox" checked={!!value} aria-label={label} onchange={(e) => { onchange(e.currentTarget.checked); ondone(); }} />
  {:else if kind === 'file'}
    <span class="grow muted name" title={value}>{value ? String(value).split('/').pop() : 'none'}</span>
    <button onclick={onpick}>Choose…</button>
  {/if}
  {#if link}
    {#if kind === 'flag' || kind === 'color' || kind === 'none'}<span class="grow"></span>{/if}
    <Source {editor} {link} {label} kind={kind === 'none' ? 'text' : kind} now={value} {min} {max} {step} {choices} />
  {/if}
</div>
{/if}
{#if source === 'setting'}
  <div class="more">Viewers change it with “{labelOf(link.raw.property, editor.project.general?.properties?.[link.raw.property])}”.</div>
{:else if source === 'keys'}
  <div class="more row">
    <button class="ghost small" disabled={before === null} onclick={() => editor.pause(before)} title="To the key before" aria-label="To the key before">◀</button>
    <button class="ghost small" class:keyed={keyed >= 0} onclick={toggleKey} title={keyed >= 0 ? 'Remove this key' : 'Add a key here'} aria-label={keyed >= 0 ? 'Remove this key' : 'Add a key here'}>{keyed >= 0 ? '◆' : '◇'}</button>
    <button class="ghost small" disabled={after === null} onclick={() => editor.pause(after)} title="To the key after" aria-label="To the key after">▶</button>
    <span>{link.raw.keys.length === 1 ? 'One key: go to another time and change it' : `${link.raw.keys.length} keys`}</span>
  </div>
{:else if source === 'audio'}
  <div class="sound">
    <Field label="Loud" {kind} value={editor.value(link.raw.to, value)} {min} {max} {step} {unit} {slider} onchange={(v) => heard('to', v)} {ondone} />
    <Field label="Listens to" kind="choice" choices={RANGES} value={link.raw.audio} onchange={(v) => heard('audio', v)} />
    <Field label="Quiet below" value={link.raw.threshold ?? 0.3} min={0} max={0.95} onchange={(v) => heard('threshold', v)} {ondone} />
    <Field label="Smoothing" value={link.raw.smooth ?? 0.1} min={0} max={2} unit="s" onchange={(v) => heard('smooth', v)} {ondone} />
  </div>
{/if}

<style>
  .field {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 30px;
  }

  .label {
    width: 92px;
    flex: none;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted);
    font-size: 13px;
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  .number {
    width: 66px;
    padding: 4px 6px;
  }

  .number.wide {
    flex: 1;
    min-width: 0;
  }

  .pair {
    appearance: textfield;
  }

  .pair::-webkit-inner-spin-button {
    appearance: none;
  }

  .unit {
    width: 24px;
    font-size: 12px;
    color: var(--muted);
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .more {
    margin: -4px 0 2px 100px;
    font-size: 11px;
    color: var(--muted);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .small {
    padding: 0 5px;
  }

  .keyed {
    color: var(--accent);
  }

  .sound {
    margin-left: 10px;
    padding-left: 8px;
    border-left: 2px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .sound :global(.label) {
    width: 84px;
  }
</style>
