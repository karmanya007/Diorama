<script>
  import { onMount } from 'svelte';
  import { app, refreshStatus, refreshCosts, scan, attempt } from './lib/state.svelte.js';
  import * as api from './lib/api.js';
  import Library from './lib/Library.svelte';
  import Monitors from './lib/Monitors.svelte';
  import Playlists from './lib/Playlists.svelte';
  import Settings from './lib/Settings.svelte';
  import Details from './lib/Details.svelte';

  const views = [
    { id: 'library', label: 'Library', icon: 'M4 5h16v2H4zm0 6h16v2H4zm0 6h10v2H4z' },
    { id: 'monitors', label: 'Monitors', icon: 'M3 5h18v11H3zm7 13h4v2h-4z' },
    { id: 'playlists', label: 'Playlists', icon: 'M4 6h12v2H4zm0 5h12v2H4zm0 5h8v2H4zm14-2 4 3-4 3z' },
    { id: 'settings', label: 'Settings', icon: 'M12 8a4 4 0 1 1 0 8 4 4 0 0 1 0-8zm8.5 4 2 1.5-2 3.5-2.4-.7a8 8 0 0 1-1.7 1L16 20h-4l-.4-2.7a8 8 0 0 1-1.7-1l-2.4.7-2-3.5 2-1.5a8 8 0 0 1 0-2l-2-1.5 2-3.5 2.4.7a8 8 0 0 1 1.7-1L12 4h4l.4 2.7a8 8 0 0 1 1.7 1l2.4-.7 2 3.5-2 1.5a8 8 0 0 1 0 2z' },
  ];
  let view = $state('library');

  onMount(() => {
    (async () => {
      await refreshStatus();
      await Promise.all([scan(), refreshCosts()]);
    })();
    const timer = setInterval(refreshStatus, 3000);
    const costTimer = setInterval(refreshCosts, 30000);
    return () => {
      clearInterval(timer);
      clearInterval(costTimer);
    };
  });

  const paused = $derived(app.status?.user_paused ?? false);
</script>

<div class="shell">
  <nav>
    <div class="brand">
      <span class="logo" aria-hidden="true"></span>
      <span>Diorama</span>
    </div>
    {#each views as v (v.id)}
      <button class="nav" class:active={view === v.id} onclick={() => (view = v.id)}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d={v.icon} /></svg>
        {v.label}
      </button>
    {/each}
    <div class="spacer"></div>
    {#if app.status}
      <button
        class="nav"
        title={paused ? 'Resume every monitor' : 'Pause every monitor'}
        onclick={() => attempt(async () => { await api.pause(!paused); await refreshStatus(); })}
      >
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d={paused ? 'M8 5v14l11-7z' : 'M7 5h4v14H7zm6 0h4v14h-4z'} />
        </svg>
        {paused ? 'Resume' : 'Pause all'}
      </button>
    {/if}
    <div class="daemon" class:bad={!!app.offline}>
      <span class="dot"></span>
      {#if app.offline}
        Daemon not reachable
      {:else if app.status}
        Running · v{app.status.daemon_version}
      {:else}
        Connecting…
      {/if}
    </div>
  </nav>

  <main>
    {#if app.offline}
      <div class="banner">
        <strong>dioramad is not running and could not be started.</strong>
        <span class="muted">{app.offline}</span>
        <button onclick={refreshStatus}>Try again</button>
      </div>
    {/if}
    {#if view === 'library'}
      <Library />
    {:else if view === 'monitors'}
      <Monitors onopen={() => (view = 'library')} />
    {:else if view === 'playlists'}
      <Playlists />
    {:else}
      <Settings />
    {/if}
  </main>

  {#if app.selected}
    <Details />
  {/if}
</div>

{#if app.toast}
  <div class="toast" role="status">{app.toast}</div>
{/if}

<style>
  .shell {
    display: grid;
    grid-template-columns: 200px 1fr auto;
    height: 100%;
  }

  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 14px 10px;
    background: var(--panel);
    border-right: 1px solid var(--line);
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 700;
    font-size: 17px;
    padding: 4px 10px 16px;
  }

  .logo {
    width: 24px;
    height: 24px;
    border-radius: 7px;
    background:
      radial-gradient(circle at 50% 44%, #ffecc8 0 28%, transparent 30%),
      linear-gradient(135deg, #18143a, #562ea0 50%, #1896aa);
  }

  .nav {
    display: flex;
    align-items: center;
    gap: 10px;
    border: none;
    background: transparent;
    text-align: left;
    padding: 8px 10px;
    color: var(--muted);
  }

  .nav svg {
    width: 18px;
    height: 18px;
    fill: currentColor;
  }

  .nav:hover {
    background: var(--panel-2);
    color: var(--text);
  }

  .nav.active {
    background: var(--accent-soft);
    color: var(--text);
    font-weight: 600;
  }

  .daemon {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px;
    font-size: 12px;
    color: var(--muted);
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ok);
  }

  .daemon.bad .dot {
    background: var(--danger);
  }

  main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .banner {
    display: flex;
    gap: 12px;
    align-items: center;
    flex-wrap: wrap;
    margin: 12px 16px 0;
    padding: 10px 14px;
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    background: var(--panel);
  }

  .toast {
    position: fixed;
    left: 50%;
    bottom: 20px;
    transform: translateX(-50%);
    max-width: 70ch;
    padding: 10px 16px;
    border-radius: 10px;
    background: var(--text);
    color: var(--bg);
    box-shadow: var(--shadow);
  }
</style>
