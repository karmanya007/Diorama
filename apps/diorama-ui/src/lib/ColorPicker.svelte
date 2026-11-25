<script>
  // A colour (`#rrggbb`): a swatch that opens a picker of our own (the
  // webviews' differ, and some have none).
  import * as api from './api.js';
  import { fromHsv, normal, toHsv } from './color.js';
  import Popover from './Popover.svelte';
  import { attempt } from './state.svelte.js';

  let { value, label = 'Colour', onchange, ondone = () => {} } = $props();

  let button;
  let field;
  let open = $state(false);
  // Kept while picking: grey has no hue, black no saturation.
  let hsv = $state({ h: 0, s: 0, v: 0 });

  const hex = $derived(normal(value) ?? '#000000');

  function show() {
    open = !open;
    if (open) hsv = toHsv(hex);
  }

  function pick(change) {
    hsv = { ...hsv, ...change };
    onchange(fromHsv(hsv));
  }

  function typed(text) {
    const color = normal(text);
    if (!color) return;
    hsv = toHsv(color);
    onchange(color);
    ondone();
  }

  async function drop() {
    const color = await attempt(() => api.pickColor());
    if (color && open) typed(color);
  }

  function place(e) {
    const r = field.getBoundingClientRect();
    const at = (v, from, size) => Math.min(1, Math.max(0, (v - from) / size));
    pick({ s: at(e.clientX, r.left, r.width), v: 1 - at(e.clientY, r.top, r.height) });
  }

  function down(e) {
    e.preventDefault();
    field.focus();
    field.setPointerCapture(e.pointerId);
    place(e);
  }

  function key(e) {
    const step = e.shiftKey ? 0.1 : 0.02;
    const moves = { ArrowLeft: ['s', -step], ArrowRight: ['s', step], ArrowUp: ['v', step], ArrowDown: ['v', -step] };
    if (!moves[e.key]) return;
    const [k, by] = moves[e.key];
    pick({ [k]: Math.min(1, Math.max(0, hsv[k] + by)) });
    e.preventDefault();
    e.stopPropagation();
  }
</script>

<button type="button" class="swatch" bind:this={button} style:background={hex} aria-label="{label}: {hex}" aria-expanded={open} title={hex} onclick={show}></button>
{#if open}
  <Popover anchor={button} width={232} height={262} onclose={() => { open = false; ondone(); }}>
    <div
      class="field"
      bind:this={field}
      role="slider"
      tabindex="0"
      aria-label="Saturation and brightness"
      aria-valuenow={Math.round(hsv.v * 100)}
      aria-valuetext={hex}
      style:background-color={fromHsv({ h: hsv.h, s: 1, v: 1 })}
      onpointerdown={down}
      onpointermove={(e) => field.hasPointerCapture(e.pointerId) && place(e)}
      onpointerup={ondone}
      onkeydown={key}
    >
      <span class="dot" style:left="{hsv.s * 100}%" style:top="{(1 - hsv.v) * 100}%" style:background={hex}></span>
    </div>
    <input class="hue" type="range" min="0" max="359" step="1" value={hsv.h} aria-label="Hue" oninput={(e) => pick({ h: Number(e.currentTarget.value) })} onchange={ondone} />
    <div class="row">
      <button type="button" class="dropper" title="Pick a colour from the screen" aria-label="Pick a colour from the screen" onclick={drop}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20.7 3.3a2.4 2.4 0 0 0-3.4 0l-3 3-1.4-1.4-1.4 1.4 1.4 1.4-8.2 8.2a2 2 0 0 0-.5.9L3 20.1a.8.8 0 0 0 .9.9l3.3-1.2c.3-.1.6-.3.9-.5l8.2-8.2 1.4 1.4 1.4-1.4-1.4-1.4 3-3a2.4 2.4 0 0 0 0-3.4zM7 17.9l-1.6.6.6-1.6 7.8-7.8 1 1z" /></svg>
      </button>
      <span class="now" style:background={hex}></span>
      <input class="grow" type="text" value={hex} maxlength="9" spellcheck="false" aria-label="Colour code" onchange={(e) => typed(e.currentTarget.value)} onkeydown={(e) => e.key === 'Enter' && typed(e.currentTarget.value)} />
    </div>
  </Popover>
{/if}

<style>
  .swatch {
    width: 40px;
    height: 28px;
    padding: 0;
    flex: none;
    border: 1px solid var(--line);
    border-radius: 6px;
    box-shadow: inset 0 0 0 2px var(--panel);
  }

  .field {
    position: relative;
    height: 150px;
    border-radius: 6px;
    cursor: crosshair;
    touch-action: none;
    background-image: linear-gradient(to top, #000, transparent), linear-gradient(to right, #fff, transparent);
  }

  .dot {
    position: absolute;
    width: 14px;
    height: 14px;
    margin: -7px 0 0 -7px;
    border: 2px solid #fff;
    border-radius: 50%;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.5);
    pointer-events: none;
  }

  .hue {
    width: 100%;
    margin: 0;
    height: 14px;
    border-radius: 7px;
    appearance: none;
    background: linear-gradient(to right, #f00, #ff0, #0f0, #0ff, #00f, #f0f, #f00);
  }

  .hue::-webkit-slider-thumb {
    appearance: none;
    width: 14px;
    height: 18px;
    border: 2px solid #fff;
    border-radius: 5px;
    background: transparent;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.5);
  }

  .dropper {
    width: 34px;
    height: 30px;
    padding: 0;
    flex: none;
    display: grid;
    place-items: center;
  }

  .dropper svg {
    width: 18px;
    height: 18px;
    fill: currentColor;
  }

  .now {
    width: 28px;
    height: 28px;
    flex: none;
    border-radius: 6px;
    border: 1px solid var(--line);
  }

  .grow {
    flex: 1;
    min-width: 0;
    font-family: ui-monospace, monospace;
  }
</style>
