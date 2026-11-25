<script>
  // One value of the document: a number (slider and box), a colour, a
  // choice, a switch or an image. Of a value that comes from a user setting
  // this is the default, and it says so.
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

{#if kind !== 'none'}
<div class="field">
  <span class="label" title={label}>{label}</span>
  {#if kind === 'number'}
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
{/if}
{#if bound}
  <div class="bound" class:alone={kind === 'none'}>
    {kind === 'none' ? `${label}: v` : 'V'}iewers can change it (setting “{bound}”).
    <button class="link" title="Takes it out of the wallpaper's settings" onclick={onunbind}>Make it fixed</button>
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

  .bound {
    margin: -4px 0 2px 100px;
    font-size: 11px;
    color: var(--muted);
  }

  .bound.alone {
    margin: 0;
  }

  .link {
    border: none;
    background: none;
    padding: 0;
    font-size: 11px;
    color: var(--accent);
  }
</style>
