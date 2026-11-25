<script>
  // The editor: layers, the picture, settings.
  import { onMount } from 'svelte';
  import { app, attempt, refreshStatus, scan, toast } from '../state.svelte.js';
  import * as api from '../api.js';
  import { Editor } from './state.svelte.js';
  import Inspector from './Inspector.svelte';
  import Layers from './Layers.svelte';
  import Leave from './Leave.svelte';
  import Stage from './Stage.svelte';

  let { dir } = $props();

  const editor = new Editor();
  let ready = $state(false);
  let failed = $state('');
  let busy = $state(false);
  let asking = $state(false);

  onMount(() => {
    const undrop = api.onDrop((paths) =>
      attempt(async () => {
        for (const path of paths) await editor.addFile(path);
      }),
    );
    editor.open(dir).then(
      () => (ready = true),
      (e) => (failed = String(e?.message ?? e)),
    );
    // What is not saved is there the next time.
    const unclose = api.onClose(() => editor.keep());
    return () => {
      for (const registered of [undrop, unclose]) registered.then((stop) => stop());
    };
  });

  async function save() {
    if (!ready || busy || editor.saving) return;
    await attempt(() => editor.save());
  }

  /** `how`: 'save', 'discard' or 'stay'; nothing: asks if it matters. */
  async function leave(how) {
    asking = !how && editor.dirty;
    if (asking || how === 'stay') return;
    busy = true;
    if (how === 'save') {
      const saved = await attempt(() => editor.save().then(() => true));
      if (!saved) return (busy = false);
    }
    if (how === 'discard') editor.discard();
    if (ready) await attempt(() => editor.close());
    // `dir` goes with this component.
    const path = editor.dir || dir;
    app.editing = null;
    await scan();
    app.selected = app.items.find((i) => i.path === path) ?? null;
  }

  async function show() {
    busy = true;
    await attempt(async () => {
      await editor.save();
      await api.setWallpaper(dir, [], undefined, 'per-output');
      await refreshStatus();
      toast('Showing on every monitor');
    });
    busy = false;
  }

  async function share() {
    busy = true;
    await attempt(async () => {
      await editor.save();
      const file = await api.pickSave(`${editor.project.title || 'wallpaper'}.diorama`);
      if (!file) return;
      await api.editorExport(dir, file);
      toast('Saved. Others open it with Library → Open file.');
    });
    busy = false;
  }

  function key(e) {
    if (ready && (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') {
      // Also while typing: what is typed counts first.
      document.activeElement?.blur?.();
      save();
      e.preventDefault();
      return;
    }
    const typing = e.target.closest?.('input:not([type=range]):not([type=checkbox]):not([type=color]), textarea, select');
    if (!ready || typing || asking) return;
    const ctrl = e.ctrlKey || e.metaKey;
    const step = e.shiftKey ? 10 : 1;
    const arrows = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] };
    const layer = editor.layer;
    if (ctrl && e.key.toLowerCase() === 'z') e.shiftKey ? editor.redo() : editor.undo();
    else if (ctrl && e.key.toLowerCase() === 'y') editor.redo();
    else if (ctrl && e.key.toLowerCase() === 'd' && layer) editor.duplicate(layer.id);
    else if ((e.key === 'Delete' || e.key === 'Backspace') && layer) editor.remove(layer.id);
    else if (e.key === 'Escape') editor.selected = null;
    else if (arrows[e.key] && !e.altKey && layer && layer.type !== 'adjustment' && !editor.bound(layer.position) && e.target.type !== 'range') {
      const [dx, dy] = arrows[e.key];
      const [x, y] = editor.place(layer).position;
      editor.edit(layer.id, (l) => (l.position = [x + dx, y + dy]), 'nudge');
    } else return;
    e.preventDefault();
  }
</script>

<svelte:window onkeydown={key} />

<div class="editor">
  <header class="row">
    <button class="ghost" onclick={() => leave()} disabled={busy}>← Library</button>
    {#if ready}
      <input
        class="title"
        type="text"
        maxlength="120"
        aria-label="Title"
        value={editor.project.title ?? ''}
        onchange={(e) => editor.changeProject((p) => (p.title = e.currentTarget.value.trim() || 'Wallpaper'))}
      />
      <span class="muted status" role="status">
        {editor.saving ? 'Saving…' : !editor.dirty ? 'Saved' : editor.restored ? 'Unsaved changes from last time' : 'Unsaved changes'}
      </span>
      {#if editor.restored && !editor.saving}
        <button class="ghost" onclick={() => editor.discard()} title="Back to the wallpaper as it is saved">Discard</button>
      {/if}
      <div class="spacer"></div>
      <button class="ghost" onclick={() => editor.undo()} disabled={!editor.past.length} title="Undo (Ctrl+Z)">Undo</button>
      <button class="ghost" onclick={() => editor.redo()} disabled={!editor.future.length} title="Redo (Ctrl+Y)">Redo</button>
      <button class="primary" onclick={save} disabled={busy || editor.saving || !editor.dirty} title="Saves the wallpaper and its settings; where it shows, it changes (Ctrl+S)">Save</button>
      <button onclick={show} disabled={busy || editor.saving} title="Saves it and shows it on every monitor">Show on desktop</button>
      <button onclick={share} disabled={busy || editor.saving} title="One file to give to others">Share…</button>
    {/if}
  </header>
  {#if failed}
    <div class="center">
      <p>This wallpaper cannot be opened.</p>
      <p class="muted">{failed}</p>
    </div>
  {:else if !ready}
    <div class="center muted">Opening…</div>
  {:else}
    <div class="panes">
      <Layers {editor} />
      <Stage {editor} />
      <Inspector {editor} />
    </div>
  {/if}
</div>

{#if asking}
  <Leave title={editor.project.title} onanswer={leave} />
{/if}

<style>
  .editor {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  header {
    padding: 8px 12px;
    background: var(--panel);
    border-bottom: 1px solid var(--line);
  }

  .title {
    width: 260px;
    font-weight: 600;
  }

  .status {
    font-size: 12px;
  }

  .panes {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  .center {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
  }
</style>
