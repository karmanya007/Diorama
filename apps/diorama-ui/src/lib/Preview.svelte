<script>
  // A wallpaper's preview. GIFs show their first frame until `live` (hover),
  // so a grid of them does not keep the CPU busy (P13).
  import { fileUrl } from './api.js';

  let { path, live = false, alt = '' } = $props();

  const url = $derived(fileUrl(path));
  const gif = $derived(/\.gif$/i.test(path ?? ''));
  let still = $state('');

  function capture(event) {
    if (!gif || still) return;
    const img = event.currentTarget;
    try {
      const canvas = document.createElement('canvas');
      canvas.width = img.naturalWidth;
      canvas.height = img.naturalHeight;
      canvas.getContext('2d').drawImage(img, 0, 0);
      still = canvas.toDataURL('image/jpeg', 0.85);
    } catch {
      // Cross-origin or decode trouble: keep the animated image.
    }
  }
</script>

{#if !path}
  <div class="empty" aria-label={alt}></div>
{:else if gif && still && !live}
  <img src={still} {alt} draggable="false" />
{:else}
  <img src={url} {alt} draggable="false" loading="lazy" onload={capture} />
{/if}

<style>
  /* Fills its (positioned) frame. */
  img,
  .empty {
    position: absolute;
    inset: 0;
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .empty {
    background:
      radial-gradient(circle at 50% 45%, var(--accent-soft) 0 20%, transparent 21%),
      var(--panel-2);
  }
</style>
