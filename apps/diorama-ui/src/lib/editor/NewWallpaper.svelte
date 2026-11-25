<script>
  // Asks for the title and size of a new wallpaper, and makes it.
  import { app, attempt } from '../state.svelte.js';
  import * as api from '../api.js';

  let { onclose } = $props();

  let dialog;
  let title = $state('My wallpaper');
  let busy = $state(false);

  const main = $derived((app.status?.outputs ?? []).find((o) => o.primary) ?? app.status?.outputs?.[0]);
  const sizes = $derived.by(() => {
    const own = main ? [[`${main.width}x${main.height}`, `${main.width} × ${main.height} (your main monitor)`]] : [];
    const usual = [['1920x1080', '1920 × 1080'], ['2560x1440', '2560 × 1440'], ['3840x2160', '3840 × 2160 (4K)'], ['1080x1920', '1080 × 1920 (upright)']];
    return [...own, ...usual.filter(([id]) => id !== own[0]?.[0])];
  });
  let size = $state('');
  const chosen = $derived(size || sizes[0][0]);

  $effect(() => {
    dialog.showModal();
  });

  async function make(e) {
    e.preventDefault();
    busy = true;
    await attempt(async () => {
      const made = await api.editorCreate(title, chosen.split('x').map(Number));
      await api.addLibraryFolder(made.folder);
      app.editing = made.dir;
      onclose();
    });
    busy = false;
  }
</script>

<dialog bind:this={dialog} onclose={() => onclose()}>
  <form onsubmit={make}>
    <h2>New wallpaper</h2>
    <label>
      <span class="muted">Title</span>
      <input type="text" bind:value={title} maxlength="60" required />
    </label>
    <label>
      <span class="muted">Size</span>
      <select value={chosen} onchange={(e) => (size = e.currentTarget.value)}>
        {#each sizes as [id, text] (id)}<option value={id}>{text}</option>{/each}
      </select>
    </label>
    <p class="muted note">It goes into Documents › Diorama. Other screens scale it.</p>
    <div class="row">
      <div class="spacer"></div>
      <button type="button" class="ghost" onclick={() => onclose()}>Cancel</button>
      <button type="submit" class="primary" disabled={busy || !title.trim()}>Make it</button>
    </div>
  </form>
</dialog>

<style>
  dialog {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
    color: var(--text);
    box-shadow: var(--shadow);
    padding: 20px;
    width: 400px;
  }

  dialog::backdrop {
    background: rgba(10, 8, 20, 0.5);
  }

  form,
  label {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  form {
    gap: 14px;
  }

  .note {
    margin: 0;
    font-size: 12px;
  }
</style>
