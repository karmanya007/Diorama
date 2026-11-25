<script>
  // The picture, and handles over it. The overlay's coordinates are the
  // wallpaper's own, from its middle.
  import { onMount } from 'svelte';
  import { contains, corners, frameSize, move, resize, rotate, round } from './geometry.js';
  import { sourceOf } from './motion.js';

  let { editor } = $props();

  // Layers with depth follow the cursor only when asked: else they are
  // where their outlines are.
  let follow = $state(false);

  let area;
  let canvas;
  let svg;
  let space = $state([0, 0]);
  let guides = $state.raw([]);
  let drag = null;
  let drawn = $state(false);

  const [W, H] = $derived(editor.size);
  // How large the picture is shown, and one screen pixel in its units.
  const shown = $derived.by(() => {
    const k = Math.min((space[0] - 32) / W, (space[1] - 32) / H);
    return k > 0 ? [W * k, H * k] : [0, 0];
  });
  const px = $derived(shown[0] ? W / shown[0] : 1);

  const layer = $derived(editor.layer);
  const box = $derived(layer && layer.type !== 'adjustment' ? editor.boxes[layer.id] : null);
  const place = $derived(layer ? editor.place(layer) : null);
  const points = $derived(box && place ? corners(place, box) : null);
  // Settings of a layer that viewers choose are not dragged.
  const fixed = $derived(!!layer && ['position', 'scale', 'rotation'].some((k) => editor.bound(layer[k])));

  const HANDLES = [[-1, -1], [1, -1], [1, 1], [-1, 1], [0, -1], [1, 0], [0, 1], [-1, 0]];
  const middle = (a, b) => [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
  const handles = $derived.by(() => {
    if (!points) return [];
    const [a, b, c, d] = points;
    const at = [a, b, c, d, middle(a, b), middle(b, c), middle(c, d), middle(d, a)];
    return HANDLES.map((side, i) => ({ side, at: at[i] }));
  });
  const turner = $derived.by(() => {
    if (!points) return null;
    const top = middle(points[0], points[1]);
    const mid = middle(points[0], points[2]);
    const [dx, dy] = [top[0] - mid[0], top[1] - mid[1]];
    const l = Math.hypot(dx, dy) || 1;
    return { from: top, at: [top[0] + (dx / l) * 28 * px, top[1] + (dy / l) * 28 * px] };
  });

  onMount(() => {
    const context = canvas.getContext('2d');
    editor.ondraw = ({ width, height, pixels }) => {
      if (canvas.width !== width || canvas.height !== height) {
        canvas.width = width;
        canvas.height = height;
      }
      context.putImageData(new ImageData(pixels, width, height), 0, 0);
      drawn = true;
    };
    const watch = new ResizeObserver(([e]) => (space = [e.contentRect.width, e.contentRect.height]));
    watch.observe(area);
    return () => {
      watch.disconnect();
      editor.ondraw = null;
    };
  });

  // Frames follow the area's size, but not every step of resizing it.
  let sizeTimer;
  $effect(() => {
    const [w, h] = shown;
    const size = [W, H];
    if (!w) return;
    clearTimeout(sizeTimer);
    sizeTimer = setTimeout(() => editor.setFrameSize(frameSize(size, [w * devicePixelRatio, h * devicePixelRatio])), 120);
  });

  function point(e) {
    const r = svg.getBoundingClientRect();
    return [((e.clientX - r.left) / r.width - 0.5) * W, ((e.clientY - r.top) / r.height - 0.5) * H];
  }

  function hit(p) {
    // The front one under the cursor; the selected one wins over those above it.
    const under = editor.layers.filter(
      (l) => editor.value(l.visible, true) !== false && editor.boxes[l.id] && l.type !== 'adjustment' && contains(editor.place(l), editor.boxes[l.id], p),
    );
    return under.find((l) => l.id === editor.selected) ?? under.at(-1) ?? null;
  }

  function down(e, what) {
    if (e.button !== 0) return;
    const p = point(e);
    if (!what) {
      const target = hit(p);
      editor.selected = target?.id ?? null;
      editor.key = null;
      if (!target || ['position'].some((k) => editor.bound(target[k]))) return;
      what = { move: true };
    } else if (fixed) {
      return;
    }
    e.stopPropagation();
    // What moves is changed where it is.
    if (['position', 'scale', 'rotation'].some((k) => sourceOf(editor.layer[k]) === 'keys')) editor.pause();
    svg.setPointerCapture(e.pointerId);
    const l = editor.layer;
    editor.settle();
    drag = { what, from: p, id: l.id, place: editor.place(l), box: editor.boxes[l.id] };
  }

  function moved(e) {
    const p = point(e);
    if (!drag) {
      if (follow) editor.pointer([p[0] / W + 0.5, p[1] / H + 0.5]);
      return;
    }
    let to;
    if (drag.what.move) {
      let [dx, dy] = [p[0] - drag.from[0], p[1] - drag.from[1]];
      // A click that shakes a little does not move anything.
      if (!drag.moved && Math.hypot(dx, dy) < 3 * px) return;
      drag.moved = true;
      if (e.shiftKey) [dx, dy] = Math.abs(dx) > Math.abs(dy) ? [dx, 0] : [0, dy];
      to = move(drag.place, drag.box, [dx, dy], [W, H], e.altKey ? 0 : 7 * px);
      guides = to.guides;
    } else if (drag.what.turn) {
      to = rotate(drag.place, drag.box, p, e.shiftKey ? 15 : 0);
    } else {
      to = resize(drag.place, drag.box, drag.what.side, p, { free: e.shiftKey, fromMiddle: e.altKey });
    }
    editor.edit(
      drag.id,
      (l) => {
        const position = to.position.map((v) => round(v));
        const old = drag.place.position;
        // Resizing and turning may leave the middle where it is.
        if (drag.what.move || position.some((v, i) => Math.abs(v - old[i]) >= 1)) editor.assign(l, 'position', position);
        if (drag.what.side) {
          const [sx, sy] = to.scale.map((v) => round(v, 4));
          editor.assign(l, 'scale', sx === sy ? sx : [sx, sy]);
        }
        if (drag.what.turn) editor.assign(l, 'rotation', round(to.rotation, 1) || (sourceOf(l.rotation) ? 0 : undefined));
      },
      'drag',
    );
  }

  function up() {
    drag = null;
    guides = [];
    editor.settle();
  }
</script>

<div class="area" bind:this={area} role="presentation" onpointerdown={(e) => e.target === area && (editor.selected = null)}>
  <div class="sheet" style:width="{shown[0]}px" style:height="{shown[1]}px">
    <canvas bind:this={canvas} class:drawn></canvas>
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <svg
      bind:this={svg}
      viewBox="{-W / 2} {-H / 2} {W} {H}"
      preserveAspectRatio="none"
      role="application"
      aria-label="Wallpaper"
      onpointerdown={(e) => down(e, null)}
      onpointermove={moved}
      onpointerup={up}
      onpointercancel={up}
      onpointerleave={() => follow && editor.pointer([0.5, 0.5])}
    >
      {#each guides as g (g.axis)}
        {#if g.axis === 0}
          <line class="guide" x1={g.at} x2={g.at} y1={-H / 2} y2={H / 2} />
        {:else}
          <line class="guide" y1={g.at} y2={g.at} x1={-W / 2} x2={W / 2} />
        {/if}
      {/each}
      {#if points}
        <polygon class="outline" points={points.map((p) => p.join(',')).join(' ')} />
        {#if !fixed}
          <line class="outline" x1={turner.from[0]} y1={turner.from[1]} x2={turner.at[0]} y2={turner.at[1]} />
          <circle class="handle turn" cx={turner.at[0]} cy={turner.at[1]} r={6 * px} role="presentation" onpointerdown={(e) => down(e, { turn: true })} />
          {#each handles as h (h.side.join())}
            <rect
              class="handle"
              x={h.at[0] - 5 * px}
              y={h.at[1] - 5 * px}
              width={10 * px}
              height={10 * px}
              role="presentation"
              onpointerdown={(e) => down(e, { side: h.side })}
            />
          {/each}
        {/if}
      {/if}
    </svg>
    {#if editor.failure}
      <div class="failure">
        <strong>No preview</strong>
        <span class="muted">{editor.failure}</span>
        <button onclick={() => editor.restart()}>Start it again</button>
      </div>
    {:else if !drawn}
      <div class="failure muted">Drawing…</div>
    {/if}
  </div>
  <label class="follow muted" title="Try the parallax: layers with a depth follow the cursor">
    <input type="checkbox" bind:checked={follow} onchange={() => follow || editor.pointer([0.5, 0.5])} />
    Follow the cursor
  </label>
</div>

<style>
  .area {
    flex: 1;
    min-width: 0;
    min-height: 0;
    position: relative;
    display: grid;
    place-items: center;
    overflow: hidden;
    background: var(--panel-2);
  }

  .sheet {
    position: relative;
    box-shadow: var(--shadow);
    background: #000;
  }

  canvas,
  svg {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    display: block;
  }

  svg {
    overflow: visible;
    touch-action: none;
  }

  .outline {
    fill: none;
    stroke: var(--accent);
    stroke-width: 1.5;
    vector-effect: non-scaling-stroke;
    pointer-events: none;
  }

  .handle {
    fill: #fff;
    stroke: var(--accent);
    stroke-width: 1.5;
    vector-effect: non-scaling-stroke;
    cursor: pointer;
  }

  .handle.turn {
    cursor: grab;
  }

  .guide {
    stroke: var(--warn);
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
  }

  .follow {
    position: absolute;
    right: 12px;
    bottom: 8px;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }

  .failure {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 20px;
    text-align: center;
    color: #ddd;
    pointer-events: none;
  }

  .failure button {
    pointer-events: auto;
    color: var(--text);
  }
</style>
