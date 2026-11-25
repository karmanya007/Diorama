<script>
  // One value of the document: a number (slider and box), a colour, a
  // choice, a switch or an image. A value that comes from a user setting
  // says so, and can be set here instead.
  import ColorPicker from '../ColorPicker.svelte';
  import { toHex } from '../props.js';

  let { label, value, bound = null, kind = 'number', min = 0, max = 1, step = 0.01, unit = '', choices = [], onchange, ondone = () => {}, onunbind = () => {}, onpick = () => {}, slider = true } = $props();

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
    if (text.trim() !== '' && Number.isFinite(n)) onchange(Math.min(max, Math.max(min, n)));
    ondone();
  }
</script>

<div class="field">
  <span class="label" title={label}>{label}</span>
  {#if bound}
    <span class="chip" title="Viewers choose it in the wallpaper's settings">setting: {bound}</span>
    <button class="ghost small" onclick={onunbind} title="Set it here instead">✕</button>
  {:else if kind === 'number'}
    {#if slider}
      <input class="grow" type="range" {min} {max} {step} value={shown} aria-label={label} oninput={(e) => onchange(Number(e.currentTarget.value))} onchange={ondone} />
    {/if}
    <input class="number" class:wide={!slider} type="number" {min} {max} {step} value={shown} aria-label={label} onchange={(e) => typed(e.currentTarget.value)} />
    {#if unit}<span class="unit">{unit}</span>{/if}
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
</div>

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

  .small {
    padding: 2px 6px;
  }
</style>
