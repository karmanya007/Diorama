<script>
  // Time: playing, standing still, and the keys of the values that move.
  import { EASES, REPEATS, lengthOf, moveKey, sorted } from './motion.js';

  let { editor } = $props();

  let open = $state(true);
  let lane = $state(0);
  // While a key is dragged the scale stays.
  let held = $state(null);
  let drag = null;

  const list = $derived.by(() => {
    const all = editor.tracks;
    // Those of the selected layer first.
    return [...all.filter((t) => t.layer.id === editor.selected), ...all.filter((t) => t.layer.id !== editor.selected)];
  });
  const span = $derived(held ?? editor.span);
  const head = $derived(editor.playing ? editor.time % editor.span : editor.time);
  const x = (t) => `${(t / span) * 100}%`;
  const ticks = $derived.by(() => {
    const step = [0.5, 1, 2, 5, 10, 30, 60, 300, 1800].find((s) => (span / s) * 46 < Math.max(lane, 200)) ?? 3600;
    return Array.from({ length: Math.floor(span / step) + 1 }, (_, i) => i * step);
  });
  const chosen = $derived(list.find((t) => t.id === editor.key?.track) ?? null);
  const key = $derived(chosen?.motion.keys[editor.key.i] ?? null);

  $effect(() => {
    if (!editor.playing) return;
    let frame = requestAnimationFrame(function step() {
      editor.tick();
      frame = requestAnimationFrame(step);
    });
    return () => cancelAnimationFrame(frame);
  });

  const clock = (t) => `${Math.floor(t / 60)}:${(t % 60).toFixed(1).padStart(4, '0')}`;

  function at(e) {
    const r = e.currentTarget.getBoundingClientRect();
    return Math.min(span, Math.max(0, ((e.clientX - r.left) / r.width) * span));
  }

  /** Near keys and whole seconds the playhead stops there. */
  function snapped(t, e, own = null) {
    if (e.altKey || !lane) return t;
    const reach = (6 / lane) * span;
    const marks = list.flatMap((l) => l.motion.keys.filter((k) => k !== own).map((k) => Number(k.t) || 0));
    const near = [...marks, ...(own ? [head] : []), Math.round(t * 2) / 2].find((m) => Math.abs(m - t) < reach);
    return near ?? t;
  }

  function scrub(e, start) {
    if (start) {
      if (e.button !== 0) return;
      e.currentTarget.setPointerCapture(e.pointerId);
      held = span;
    } else if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
    editor.pause(snapped(at(e), e));
  }

  function grab(e, track, i) {
    if (e.button !== 0) return;
    e.stopPropagation();
    e.currentTarget.setPointerCapture(e.pointerId);
    held = span;
    editor.settle();
    editor.selected = track.layer.id;
    editor.key = { track: track.id, i };
    editor.pause(Number(track.motion.keys[i].t) || 0);
    drag = { id: track.id, i, from: e.clientX, moved: false };
  }

  function pull(e) {
    if (!drag || !e.currentTarget.hasPointerCapture(e.pointerId)) return;
    if (!drag.moved && Math.abs(e.clientX - drag.from) < 3) return;
    drag.moved = true;
    const r = e.currentTarget.parentElement.getBoundingClientRect();
    const own = list.find((t) => t.id === drag.id)?.motion.keys[drag.i];
    const t = snapped(Math.max(0, ((e.clientX - r.left) / r.width) * span), e, own);
    editor.editTrack(drag.id, (motion) => moveKey(motion, drag.i, t), 'key');
    editor.pause(Number(list.find((l) => l.id === drag.id)?.motion.keys[drag.i]?.t) || 0);
  }

  function drop() {
    drag = null;
    held = null;
    editor.settle();
  }

  const setTime = (text) => {
    const t = Number(text);
    if (text.trim() === '' || !Number.isFinite(t)) return;
    editor.editTrack(editor.key.track, (m) => moveKey(m, editor.key.i, t));
    editor.pause(Number(key?.t) || 0);
  };
  const setEase = (v) => editor.editTrack(editor.key.track, (m) => (m.keys[editor.key.i].ease = v === 'in-out' ? undefined : v));
  const setRepeat = (v) => editor.editTrack(chosen.id, (m) => (m.repeat = v === 'loop' ? undefined : v));
  const setLength = (text) => {
    const n = Number(text);
    if (text.trim() === '' || !Number.isFinite(n)) return;
    const last = Number(sorted(chosen.motion).at(-1)?.t) || 0;
    editor.editTrack(chosen.id, (m) => (m.length = n > last ? Math.min(86400, n) : undefined));
  };
</script>

<div class="timeline">
  <div class="bar">
    <button class="ghost icon" onclick={() => (editor.playing ? editor.pause() : editor.play())} title={editor.playing ? 'Pause (Space)' : 'Play (Space)'} aria-label={editor.playing ? 'Pause' : 'Play'}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d={editor.playing ? 'M6 5h4v14H6zm8 0h4v14h-4z' : 'M7 4.5v15l13-7.5z'} /></svg>
    </button>
    <button class="ghost icon" onclick={() => { const on = editor.playing; editor.pause(0); if (on) editor.play(); }} title="To the start" aria-label="To the start">
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M5 5h3v14H5zm14 0v14L9 12z" /></svg>
    </button>
    <code>{clock(head)}</code>
    {#if key}
      <span class="sep"></span>
      <label>Key at <input class="number" type="number" min="0" step="0.1" value={Number(key.t) || 0} aria-label="Time of the key" onchange={(e) => setTime(e.currentTarget.value)} /> s</label>
      <select value={key.ease ?? 'in-out'} aria-label="How it goes on" title="How it goes on to the next key" onchange={(e) => setEase(e.currentTarget.value)}>
        {#each EASES as [id, text] (id)}<option value={id}>{text}</option>{/each}
      </select>
      <button class="ghost danger" onclick={() => editor.removeKey()} title="Removes the key (Delete)">Remove key</button>
      <span class="sep"></span>
      <select value={chosen.motion.repeat ?? 'loop'} aria-label="At the end" title="What happens at the end" onchange={(e) => setRepeat(e.currentTarget.value)}>
        {#each REPEATS as [id, text] (id)}<option value={id}>{text}</option>{/each}
      </select>
      <label title="With time after the last key, one that starts over goes back to its first key">after <input class="number" type="number" min="0" step="0.5" value={Number(lengthOf(chosen.motion).toFixed(2))} aria-label="Length" onchange={(e) => setLength(e.currentTarget.value)} /> s</label>
    {:else if !list.length}
      <span class="muted hint">To make something move, press ◇ beside its value.</span>
    {/if}
    <span class="spacer"></span>
    {#if list.length}
      <button class="ghost icon" onclick={() => (open = !open)} aria-expanded={open} title={open ? 'Hide the keys' : 'Show the keys'} aria-label="Keys">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d={open ? 'M5 9l7 7 7-7z' : 'M5 15l7-7 7 7z'} /></svg>
      </button>
    {/if}
  </div>
  {#if list.length && open}
    <div class="rows scroll">
      <div class="row ruler">
        <span class="name"></span>
        <div
          class="lane"
          bind:clientWidth={lane}
          role="slider"
          tabindex="0"
          aria-label="Time"
          aria-valuemin="0"
          aria-valuemax={span}
          aria-valuenow={Number(head.toFixed(1))}
          onpointerdown={(e) => scrub(e, true)}
          onpointermove={(e) => scrub(e, false)}
          onpointerup={() => (held = null)}
          onkeydown={(e) => {
            const by = { ArrowLeft: -0.1, ArrowRight: 0.1 }[e.key];
            if (!by) return;
            editor.pause(Math.min(span, Math.max(0, head + by)));
            e.preventDefault();
            e.stopPropagation();
          }}
        >
          {#each ticks as t (t)}<span class="tick" style:left={x(t)}>{t >= 60 ? clock(t) : t}</span>{/each}
          <span class="head top" style:left={x(head)}></span>
        </div>
      </div>
      {#each list as track (track.id)}
        {@const dim = editor.selected && track.layer.id !== editor.selected}
        <div class="row" class:dim>
          <button class="ghost name" title="{track.layer.name || track.layer.type} · {track.label}" onclick={() => (editor.selected = track.layer.id)}>
            {track.layer.name || track.layer.type} · {track.label}
          </button>
          <div class="lane">
            <span class="extent" style:width={x(Math.min(span, lengthOf(track.motion)))}></span>
            {#each track.motion.keys as k, i (i)}
              <button
                class="key"
                class:on={editor.key?.track === track.id && editor.key.i === i}
                style:left={x(Number(k.t) || 0)}
                title="{Number(k.t) || 0} s"
                aria-label="Key at {Number(k.t) || 0} seconds"
                onpointerdown={(e) => grab(e, track, i)}
                onpointermove={pull}
                onpointerup={drop}
                onpointercancel={drop}
                onclick={(e) => {
                  // By keyboard.
                  if (e.detail) return;
                  editor.selected = track.layer.id;
                  editor.key = { track: track.id, i };
                  editor.pause(Number(k.t) || 0);
                }}
              ></button>
            {/each}
            <span class="head" style:left={x(head)}></span>
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .timeline {
    flex: none;
    background: var(--panel);
    border-top: 1px solid var(--line);
    font-size: 12px;
  }

  .bar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    padding: 4px 8px;
    min-height: 38px;
  }

  .bar label {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--muted);
  }

  .bar select,
  .bar button {
    padding: 3px 8px;
  }

  .number {
    width: 62px;
    padding: 3px 6px !important;
    color: var(--text);
  }

  .icon {
    display: grid;
    place-items: center;
  }

  .icon svg {
    width: 16px;
    height: 16px;
    fill: currentColor;
  }

  .hint {
    margin-left: 6px;
  }

  .sep {
    width: 1px;
    height: 18px;
    background: var(--line);
    margin: 0 4px;
  }

  .spacer {
    flex: 1;
  }

  .rows {
    max-height: 170px;
    padding-bottom: 6px;
  }

  .row {
    display: flex;
    align-items: center;
    height: 24px;
    padding-right: 16px;
  }

  .row.dim .key,
  .row.dim .name {
    opacity: 0.55;
  }

  .ruler {
    position: sticky;
    top: 0;
    z-index: 2;
    background: var(--panel);
  }

  .name {
    width: 170px;
    flex: none;
    padding: 0 10px;
    border: none;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
  }

  .lane {
    position: relative;
    flex: 1;
    min-width: 0;
    height: 100%;
    touch-action: none;
  }

  .ruler .lane {
    cursor: pointer;
    border-bottom: 1px solid var(--line);
  }

  .tick {
    position: absolute;
    bottom: 2px;
    transform: translateX(-50%);
    color: var(--muted);
    font-size: 11px;
    pointer-events: none;
  }

  .extent {
    position: absolute;
    left: 0;
    top: 11px;
    height: 2px;
    background: var(--line);
  }

  .head {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 1px;
    background: var(--danger);
    pointer-events: none;
  }

  .head.top::before {
    content: '';
    position: absolute;
    left: -5px;
    top: 0;
    border: 5px solid transparent;
    border-top-color: var(--danger);
  }

  .key {
    position: absolute;
    top: 6px;
    width: 12px;
    height: 12px;
    margin-left: -6px;
    padding: 0;
    border: 1.5px solid var(--accent);
    border-radius: 2px;
    background: var(--panel);
    transform: rotate(45deg);
    z-index: 1;
    cursor: ew-resize;
    touch-action: none;
  }

  .key.on {
    background: var(--accent);
  }
</style>
