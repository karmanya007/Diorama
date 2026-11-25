<script>
  // Where a value comes from: fixed, keys, the sound, or a setting viewers
  // change. `link`: { raw, set, moves, key } of the value in the document.
  import Popover from '../Popover.svelte';
  import { fromHex } from '../props.js';
  import { labelOf } from '../props.js';
  import { louder, sourceOf } from './motion.js';

  let { editor, link, label, kind, now, min, max, step, choices } = $props();

  let button;
  let open = $state(false);
  let name = $state('');

  const source = $derived(sourceOf(link.raw));
  const TYPES = { number: 'slider', color: 'color', flag: 'bool', choice: 'combo', text: 'textinput' };
  const type = $derived(TYPES[kind]);
  const known = $derived(type ? editor.settings.filter((s) => s.type === type) : []);

  const ICON = {
    keys: 'M12 3l7 9-7 9-7-9z',
    audio: 'M4 10h3v4H4zm4-4h3v12H8zm4 3h3v6h-3zm4-5h3v16h-3z',
    setting: 'M4 6h9v2H4zm11-2h2v6h-2zm2 2h3v2h-3zM4 15h3v2H4zm3-2h2v6H7zm2 2h11v2H9z',
  };
  const TITLE = { keys: 'It moves between keys', audio: 'It follows the sound', setting: 'Viewers can change it' };

  function show() {
    open = !open;
    name = label;
  }

  function choose(raw) {
    open = false;
    link.set(raw);
  }

  const plain = () => (now === undefined ? null : JSON.parse(JSON.stringify(now)));

  function keys() {
    editor.pause(0);
    choose({ keys: [{ t: 0, v: plain() }], repeat: 'mirror' });
  }

  function audio() {
    choose({ audio: 'bass', from: plain(), to: louder(plain(), kind, link.key, { min, max }) });
  }

  function definition() {
    const value = plain();
    if (kind === 'color') return { type, value: /^#/.test(value ?? '') ? fromHex(value) : value };
    if (kind === 'flag') return { type, value: !!value };
    if (kind === 'choice') return { type, options: choices.map(([v, text]) => ({ label: text, value: v })), value };
    if (kind === 'text') return { type, value: String(value ?? '') };
    return { type, min, max, step, value: Number(value) || 0 };
  }

  function make(e) {
    e.preventDefault();
    const text = name.trim() || label;
    open = false;
    // The setting and its use are one to undo.
    link.set(() => ({ property: editor.makeSetting(text, definition()) }));
  }
</script>

<button
  type="button"
  class="source ghost"
  class:on={source}
  bind:this={button}
  onclick={show}
  aria-expanded={open}
  aria-label="{label}: where it comes from"
  title={TITLE[source] ?? (link.moves ? 'Make it move, or let viewers change it' : 'Let viewers change it')}
>
  <svg viewBox="0 0 24 24" aria-hidden="true"><path d={ICON[source] ?? (link.moves ? ICON.keys : ICON.setting)} /></svg>
</button>
{#if open}
  <Popover anchor={button} width={230} height={340} onclose={() => (open = false)}>
    <strong class="what">{label}</strong>
    <button class="item" class:on={!source} onclick={() => choose(plain())}>Fixed</button>
    {#if link.moves}
      <button class="item" class:on={source === 'keys'} onclick={() => (source === 'keys' ? (open = false) : keys())}>Moves between keys</button>
      <button class="item" class:on={source === 'audio'} onclick={() => (source === 'audio' ? (open = false) : audio())}>Follows the sound</button>
    {/if}
    {#if type}
      <span class="muted head">Viewers change it</span>
      <div class="list">
        {#each known as s (s.name)}
          <button class="item" class:on={link.raw?.property === s.name} onclick={() => choose({ property: s.name })}>With “{labelOf(s.name, s.def)}”</button>
        {/each}
      </div>
      <form class="row" onsubmit={make}>
        <input type="text" bind:value={name} maxlength="60" aria-label="Name of the setting" placeholder="New setting" />
        <button type="submit">Add</button>
      </form>
    {/if}
  </Popover>
{/if}

<style>
  .source {
    flex: none;
    width: 24px;
    height: 24px;
    padding: 0;
    display: grid;
    place-items: center;
  }

  .source svg {
    width: 14px;
    height: 14px;
    fill: none;
    stroke: var(--muted);
    stroke-width: 1.8;
  }

  .source.on svg {
    fill: var(--accent);
    stroke: none;
  }

  .what {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .item {
    text-align: left;
    border-color: transparent;
    padding: 4px 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: none;
  }

  .item.on {
    background: var(--accent-soft);
  }

  .head {
    font-size: 12px;
  }

  .list {
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    min-height: 0;
  }

  form input {
    flex: 1;
    min-width: 0;
  }
</style>
