<script>
  // Asks what to do with unsaved changes before leaving the editor.
  let { title, onanswer } = $props();

  let dialog;

  $effect(() => {
    dialog.showModal();
  });

  function answer(how) {
    dialog.close();
    onanswer(how);
  }
</script>

<dialog bind:this={dialog} oncancel={(e) => { e.preventDefault(); answer('stay'); }}>
  <h2>Save the changes to “{title || 'Wallpaper'}”?</h2>
  <div class="row">
    <button class="ghost" onclick={() => answer('stay')}>Keep editing</button>
    <div class="spacer"></div>
    <button class="danger" onclick={() => answer('discard')}>Don’t save</button>
    <button class="primary" onclick={() => answer('save')}>Save</button>
  </div>
</dialog>

<style>
  dialog {
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--panel);
    color: var(--text);
    box-shadow: var(--shadow);
    padding: 20px;
    width: 420px;
  }

  dialog::backdrop {
    background: rgba(10, 8, 20, 0.5);
  }

  .row {
    margin-top: 18px;
  }
</style>
