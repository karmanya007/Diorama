<script>
  // The layers, front to back, and adding new ones.
  import { attempt } from '../state.svelte.js';
  import * as api from '../api.js';

  let { editor } = $props();

  const ICONS = {
    image: 'M4 5h16v14H4zm2 12h12l-4-6-3 4-2-2z',
    video: 'M4 6h11v12H4zm12 4 4-3v10l-4-3z',
    text: 'M5 5h14v4h-2V7h-4v10h2v2H9v-2h2V7H7v2H5z',
    color: 'M12 3s6 7 6 11a6 6 0 0 1-12 0c0-4 6-11 6-11z',
    adjustment: 'M4 7h9v2H4zm11-2h2v6h-2zm2 2h3v2h-3zM4 15h3v2H4zm3-2h2v6H7zm2 2h11v2H9z',
  };

  // Front first, as in other editors; the document counts from the back.
  const list = $derived([...editor.layers].reverse());
  let renaming = $state(null);
  let dragging = $state(null);
  let over = $state(null);

  const name = (l) => l.name || { image: 'Image', video: 'Video', text: 'Text', color: 'Colour', adjustment: 'Adjustment' }[l.type] || 'Layer';

  const files = (kind) =>
    attempt(async () => {
      const paths = await api.pickFiles(kind);
      for (const path of paths) await editor.addFile(path);
    });

  const add = {
    text: () => editor.add({ name: 'Text', type: 'text', text: 'Text', color: '#ffffff', 'font-size': Math.round(editor.size[1] / 12), position: [0, 0] }),
    color: () => editor.add({ name: 'Colour', type: 'color', color: '#4060a0', size: editor.size.map((v) => Math.round(v / 3)), position: [0, 0] }),
    adjustment: () => editor.add({ name: 'Adjustment', type: 'adjustment', effects: [{ type: 'vignette' }] }),
  };

  function drop(target) {
    if (dragging && target && dragging !== target) {
      editor.reorder(
        dragging,
        editor.layers.findIndex((l) => l.id === target),
      );
    }
    dragging = over = null;
  }

  function rename(layer, text) {
    renaming = null;
    editor.edit(layer.id, (l) => (l.name = text.trim() || undefined));
  }
</script>

<aside>
  <div class="add">
    <button onclick={() => files('image')} title="Add pictures">Image</button>
    <button onclick={() => files('video')} title="Add a video">Video</button>
    <button onclick={add.text}>Text</button>
    <button onclick={add.color}>Colour</button>
    <button onclick={add.adjustment} title="Effects over everything below">Adjustment</button>
  </div>

  <div class="list scroll" role="listbox" aria-label="Layers">
    {#each list as layer (layer.id)}
      <div
        class="layer"
        role="option"
        tabindex="0"
        aria-selected={editor.selected === layer.id}
        class:on={editor.selected === layer.id}
        class:hidden={editor.value(layer.visible, true) === false}
        class:over={over === layer.id}
        draggable={renaming !== layer.id}
        ondragstart={(e) => { dragging = layer.id; e.dataTransfer.effectAllowed = 'move'; e.dataTransfer.setData('text/plain', layer.id); }}
        ondragover={(e) => { e.preventDefault(); over = layer.id; }}
        ondragend={() => (dragging = over = null)}
        ondrop={(e) => { e.preventDefault(); drop(layer.id); }}
        onclick={() => (editor.selected = layer.id)}
        ondblclick={() => (renaming = layer.id)}
        onkeydown={(e) => {
          if (e.target !== e.currentTarget) return;
          const i = editor.layers.findIndex((l) => l.id === layer.id);
          if (e.key === 'Enter') editor.selected = layer.id;
          else if (e.key === 'F2') renaming = layer.id;
          else if (e.altKey && e.key === 'ArrowUp') editor.reorder(layer.id, i + 1);
          else if (e.altKey && e.key === 'ArrowDown') editor.reorder(layer.id, i - 1);
          else return;
          e.preventDefault();
          e.stopPropagation();
        }}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d={ICONS[layer.type] ?? ICONS.image} /></svg>
        {#if renaming === layer.id}
          <!-- svelte-ignore a11y_autofocus -->
          <input
            class="grow"
            type="text"
            value={name(layer)}
            maxlength="80"
            autofocus
            aria-label="Layer name"
            onblur={(e) => rename(layer, e.currentTarget.value)}
            onkeydown={(e) => { if (e.key === 'Enter') e.currentTarget.blur(); if (e.key === 'Escape') renaming = null; e.stopPropagation(); }}
          />
        {:else}
          <span class="grow name" title={name(layer)}>{name(layer)}</span>
          {#if layer.effects?.length}<span class="chip" title="Effects">{layer.effects.length}</span>{/if}
          <button
            class="ghost small"
            title={editor.value(layer.visible, true) === false ? 'Show' : 'Hide'}
            aria-label={editor.value(layer.visible, true) === false ? 'Show' : 'Hide'}
            onclick={(e) => { e.stopPropagation(); editor.edit(layer.id, (l) => (l.visible = editor.value(layer.visible, true) === false ? undefined : false)); }}
          >
            {editor.value(layer.visible, true) === false ? '○' : '●'}
          </button>
        {/if}
      </div>
    {:else}
      <p class="muted pad">Add a picture to start, or drop files here.</p>
    {/each}
  </div>

  <div class="tools">
    <button disabled={!editor.layer} onclick={() => editor.reorder(editor.selected, editor.layers.length)} title="Bring to the front">Front</button>
    <button disabled={!editor.layer} onclick={() => editor.reorder(editor.selected, 0)} title="Send to the back">Back</button>
    <button disabled={!editor.layer} onclick={() => editor.duplicate(editor.selected)} title="Duplicate (Ctrl+D)">Copy</button>
    <button class="danger" disabled={!editor.layer} onclick={() => editor.remove(editor.selected)} title="Delete (Del)">Delete</button>
  </div>
</aside>

<style>
  aside {
    width: 236px;
    display: flex;
    flex-direction: column;
    min-height: 0;
    background: var(--panel);
    border-right: 1px solid var(--line);
  }

  .add,
  .tools {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 10px;
  }

  .add button,
  .tools button {
    flex: 1 1 auto;
    padding: 5px 8px;
    font-size: 13px;
  }

  .add {
    border-bottom: 1px solid var(--line);
  }

  .tools {
    border-top: 1px solid var(--line);
  }

  .list {
    flex: 1;
    min-height: 0;
    padding: 6px;
  }

  .layer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border-radius: 8px;
    border-top: 2px solid transparent;
    cursor: default;
  }

  .layer:hover {
    background: var(--panel-2);
  }

  .layer.on {
    background: var(--accent-soft);
  }

  .layer.over {
    border-top-color: var(--accent);
  }

  .layer.hidden .name,
  .layer.hidden svg {
    opacity: 0.45;
  }

  .layer:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  svg {
    width: 16px;
    height: 16px;
    flex: none;
    fill: var(--muted);
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .small {
    padding: 0 6px;
  }

  .pad {
    padding: 10px;
  }
</style>
