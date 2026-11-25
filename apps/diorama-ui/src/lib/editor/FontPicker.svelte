<script>
  // The font of a text layer: an installed one by name, or a file that goes
  // with the wallpaper.
  import { attempt, toast } from '../state.svelte.js';
  import * as api from '../api.js';
  import Popover from '../Popover.svelte';

  let { editor, layer } = $props();

  const SYSTEM = 'system:';
  // Names in fonts without letters could not be read.
  const SYMBOLS = /dings|symbol|icon|emoji|mdl2|marlett|standard symbols/i;
  const sample = (f) => (f && !SYMBOLS.test(f) ? `"${f}"` : null);

  let button;
  let open = $state(false);
  let query = $state('');
  let search = $state();

  const font = $derived(String(editor.value(layer.font, '') ?? ''));
  const family = $derived(font.startsWith(SYSTEM) ? font.slice(SYSTEM.length) : null);
  const shown = $derived(family ?? (font ? font.split('/').pop() : 'Default'));
  // Files that texts of this wallpaper use already.
  const files = $derived([...new Set(editor.layers.map((l) => l.font).filter((f) => typeof f === 'string' && f && !f.startsWith(SYSTEM)))]);
  const found = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return editor.fonts.filter((f) => !q || f.toLowerCase().includes(q));
  });

  $effect(() => {
    if (open) search?.focus();
  });

  function set(value) {
    open = false;
    editor.edit(layer.id, (l) => {
      l.font = value || undefined;
      // Files are one face; installed fonts have several.
      if (!value?.startsWith(SYSTEM)) l.bold = l.italic = undefined;
    });
  }

  const flag = (key) => editor.edit(layer.id, (l) => (l[key] = l[key] ? undefined : true));

  async function choose() {
    open = false;
    await attempt(async () => {
      const [path] = await api.pickFiles('font', false);
      if (path) set(await api.editorAddAsset(editor.dir, path));
    });
  }

  async function include() {
    await attempt(async () => {
      set(await api.editorAddFont(editor.dir, family, !!layer.bold, !!layer.italic));
      toast('The font file is in the wallpaper now');
    });
  }
</script>

<div class="field">
  <span class="label">Font</span>
  <button class="current" bind:this={button} aria-expanded={open} onclick={() => { open = !open; query = ''; }}>
    <span class="name" style:font-family={sample(family)}>{shown}</span>
    <span aria-hidden="true">▾</span>
  </button>
  {#if family !== null}
    <button class="style" class:on={layer.bold} aria-pressed={!!layer.bold} title="Bold" onclick={() => flag('bold')}><b>B</b></button>
    <button class="style" class:on={layer.italic} aria-pressed={!!layer.italic} title="Italic" onclick={() => flag('italic')}><i>I</i></button>
  {/if}
</div>
{#if family !== null}
  <p class="note">
    Others see {family} only if they have it.
    <button class="link" onclick={include}>Put its file in the wallpaper</button> if its licence lets you share it.
  </p>
{:else if !font}
  <p class="note">The default differs between computers.</p>
{/if}

{#if open}
  <Popover anchor={button} width={280} height={360} onclose={() => (open = false)}>
    <input type="search" placeholder="Search fonts" bind:value={query} bind:this={search} aria-label="Search fonts" onkeydown={(e) => e.key === 'Enter' && found.length && set(SYSTEM + found[0])} />
    <div class="list scroll" role="listbox" aria-label="Fonts">
      {#if !query}
        <button role="option" aria-selected={!font} class:on={!font} onclick={() => set('')}>Default</button>
        {#each files as f (f)}
          <button role="option" aria-selected={font === f} class:on={font === f} onclick={() => set(f)}>{f.split('/').pop()} <span class="muted">(in the wallpaper)</span></button>
        {/each}
      {/if}
      {#each found as f (f)}
        <button role="option" aria-selected={family === f} class:on={family === f} title={f} style:font-family={sample(f)} onclick={() => set(SYSTEM + f)}>{f}</button>
      {:else}
        <p class="muted">{editor.fonts.length ? 'No font of that name.' : 'Looking for fonts…'}</p>
      {/each}
    </div>
    <button onclick={choose}>Choose a font file…</button>
  </Popover>
{/if}

<style>
  .field {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 30px;
  }

  .label {
    width: 92px;
    flex: none;
    color: var(--muted);
    font-size: 13px;
    margin-right: 2px;
  }

  .current {
    flex: 1;
    min-width: 0;
    display: flex;
    gap: 6px;
    justify-content: space-between;
    text-align: left;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .style {
    width: 30px;
    padding: 5px 0;
    flex: none;
  }

  .style.on {
    background: var(--accent-soft);
    border-color: var(--accent);
  }

  .list {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .list button {
    flex: none;
    border: none;
    background: transparent;
    text-align: left;
    padding: 5px 8px;
    font-size: 15px;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .list button:hover {
    background: var(--panel-2);
  }

  .list button.on {
    background: var(--accent-soft);
  }

  .note {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
  }

  .link {
    border: none;
    background: none;
    padding: 0;
    font-size: 12px;
    color: var(--accent);
    white-space: normal;
  }
</style>
