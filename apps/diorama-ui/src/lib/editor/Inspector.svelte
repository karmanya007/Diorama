<script>
  // Settings of the selected layer, or of the wallpaper.
  import { attempt } from '../state.svelte.js';
  import * as api from '../api.js';
  import Field from './Field.svelte';
  import FontPicker from './FontPicker.svelte';
  import { sourceOf } from './motion.js';
  import Source from './Source.svelte';
  import ViewerSettings from './ViewerSettings.svelte';

  let { editor } = $props();

  const BLENDS = ['normal', 'add', 'multiply', 'screen', 'overlay', 'darken', 'lighten', 'hard-light', 'soft-light', 'difference'].map((b) => [b, b[0].toUpperCase() + b.slice(1).replace('-', ' ')]);
  const SIZES = [
    ['1920x1080', '1920 × 1080'],
    ['2560x1440', '2560 × 1440'],
    ['3840x2160', '3840 × 2160 (4K)'],
    ['3440x1440', '3440 × 1440 (wide)'],
    ['1080x1920', '1080 × 1920 (upright)'],
  ];
  const ALIGN = [['left', 'Left'], ['center', 'Centre'], ['right', 'Right']];
  const VALIGN = [['top', 'Top'], ['middle', 'Middle'], ['bottom', 'Bottom']];
  const GROUPS = { blur: 'Blur', color: 'Colour', style: 'Style', motion: 'Motion', shape: 'Shape' };

  const layer = $derived(editor.layer);
  const place = $derived(layer ? editor.place(layer) : null);
  const placed = $derived(layer && layer.type !== 'adjustment');
  const sized = $derived(layer && ['image', 'video', 'color'].includes(layer.type));
  const groups = $derived(Object.entries(GROUPS).map(([id, label]) => [label, editor.effects.filter((e) => e.group === id)]));

  // Leaves out what is as it would be without it.
  const set = (key, otherwise) => (v) =>
    editor.edit(layer.id, (l) => editor.assign(l, key, JSON.stringify(v) === JSON.stringify(otherwise) && !sourceOf(l[key]) ? undefined : v), key);
  // `raw`: what the document is to hold (or makes it, within the change).
  const made = (raw) => (typeof raw === 'function' ? raw() : raw);
  const link = (key, moves = true) => ({ key, moves, raw: layer[key], set: (raw, kind) => editor.edit(layer.id, (l) => (l[key] = made(raw)), kind && `${key} ${kind}`) });
  const done = () => editor.settle();
  const get = (key, otherwise) => editor.value(layer[key], otherwise);

  function both(v) {
    // Proportions stay: the larger side says how much.
    const [sx, sy] = place.scale;
    const k = sx ? v / sx : 1;
    set('scale', 1)(Math.abs(sx) === Math.abs(sy) ? (sx === sy ? v : [v, sy * k]) : [v, sy * k]);
  }

  const size = $derived.by(() => {
    const b = layer && editor.boxes[layer.id];
    return b ? [Math.round(b.width), Math.round(b.height)] : [0, 0];
  });
  const resizeTo = (i, v) => {
    const now = [...size];
    now[i] = Math.max(1, Math.round(v));
    editor.edit(layer.id, (l) => (l.size = now), 'size');
  };

  const pickAsset = (kind, fn) =>
    attempt(async () => {
      const [path] = await api.pickFiles(kind, false);
      if (path) fn(await api.editorAddAsset(editor.dir, path));
    });

  // ---- effects ---
  const effectOf = (name) => editor.effects.find((e) => e.name === name);
  const edits = (i, fn, kind) => editor.edit(layer.id, (l) => fn(l.effects[i]), kind && `${kind} ${i}`);
  const otherwise = (s) => s.default ?? null;
  // All but the speed an effect moves with can move.
  const effectLink = (i, s, e, effect) => ({
    key: s.key,
    moves: ['number', 'color'].includes(s.type) && effect.animated !== s.key,
    raw: e[s.key],
    set: (raw, kind) => edits(i, (x) => (x[s.key] = made(raw)), kind && `${s.key} ${kind}`),
  });

  async function addEffect(name) {
    const effect = effectOf(name);
    if (!effect) return;
    const made = { type: name };
    // One that cannot do without an image asks for it first.
    for (const s of effect.settings.filter((s) => s.type === 'image')) {
      const path = await attempt(async () => (await api.pickFiles('image', false))[0]);
      if (!path) return;
      made[s.key] = await attempt(() => api.editorAddAsset(editor.dir, path));
      if (!made[s.key]) return;
    }
    editor.edit(layer.id, (l) => (l.effects = [...(l.effects ?? []), made]));
  }

  function moveEffect(i, by) {
    editor.edit(layer.id, (l) => {
      const j = i + by;
      if (j < 0 || j >= l.effects.length) return;
      [l.effects[i], l.effects[j]] = [l.effects[j], l.effects[i]];
    });
  }

  // ---- wallpaper ---
  const wall = (key, v) => editor.change((doc) => editor.assign(doc, key, v), key);
  const wallLink = (key) => ({ key, moves: false, raw: editor.doc[key], set: (raw) => editor.change((doc) => (doc[key] = made(raw))) });
  const parallax = $derived(editor.doc?.parallax ?? null);
  const setParallax = (key, v) =>
    editor.change((doc) => {
      doc.parallax ??= {};
      editor.assign(doc.parallax, key, v);
    }, `parallax ${key}`);
  const sizeId = $derived(editor.size.join('x'));
</script>

<aside class="scroll">
  {#if layer}
    <h2>{layer.name || layer.type}</h2>

    {#if layer.type === 'text'}
      <section>
        <div class="words">
          <textarea rows="2" maxlength="10000" aria-label="Text" value={get('text', '')} oninput={(e) => set('text', '')(e.currentTarget.value)} onchange={done}></textarea>
          <Source {editor} link={link('text', false)} label="Text" kind="text" now={get('text', '')} />
        </div>
        {#if editor.bound(layer.text)}<p class="note">Viewers can change the text.</p>{/if}
        <Field label="Size" value={get('font-size', 64)} {editor} link={link('font-size', false)} min={1} max={1000} step={1} unit="px" onchange={set('font-size', 64)} ondone={done} />
        <Field label="Colour" kind="color" value={get('color', '#ffffff')} {editor} link={link('color')} onchange={set('color', '#ffffff')} ondone={done} />
        <FontPicker {editor} {layer} />
        <Field label="Align" kind="choice" choices={ALIGN} value={get('align', 'center')} onchange={set('align', 'center')} />
        <Field label="Vertical" kind="choice" choices={VALIGN} value={get('vertical-align', 'middle')} onchange={set('vertical-align', 'middle')} />
        <Field label="Max width" value={get('max-width', 0)} min={0} max={editor.size[0] * 2} step={10} unit="px" onchange={(v) => set('max-width', 0)(v)} ondone={done} />
      </section>
    {:else if layer.type === 'image' || layer.type === 'video'}
      <section>
        <Field label="File" kind="file" value={layer.source} onpick={() => pickAsset(layer.type, set('source'))} />
        <Field label="Tint" kind="color" value={get('tint', '#ffffff')} {editor} link={link('tint')} onchange={set('tint', '#ffffff')} ondone={done} />
        {#if layer.type === 'image'}
          <Field label="Smooth" kind="flag" value={get('smooth', true)} onchange={set('smooth', true)} />
        {/if}
      </section>
    {:else if layer.type === 'color'}
      <section>
        <Field label="Colour" kind="color" value={get('color', '#ffffff')} {editor} link={link('color')} onchange={set('color', '#ffffff')} ondone={done} />
      </section>
    {/if}

    {#if placed}
      <section>
        <h3>Place</h3>
        <Field label="Position" kind="pair" {editor} link={link('position')} value={place.position} min={-100000} max={100000} step={1} unit="px" onchange={(v) => set('position')(v)} ondone={done} />
        {#if sized}
          <Field label="Width" slider={false} value={size[0]} min={1} max={16384} step={1} unit="px" onchange={(v) => resizeTo(0, v)} ondone={done} />
          <Field label="Height" slider={false} value={size[1]} min={1} max={16384} step={1} unit="px" onchange={(v) => resizeTo(1, v)} ondone={done} />
        {/if}
        <Field label="Scale" value={place.scale[0]} {editor} link={link('scale')} min={0.01} max={5} step={0.01} onchange={both} ondone={done} />
        <Field label="Rotation" value={place.rotation} {editor} link={link('rotation')} min={-180} max={180} limit={[-36000, 36000]} step={1} unit="°" onchange={set('rotation', 0)} ondone={done} />
        <Field label="Depth" value={get('depth', 0)} {editor} link={link('depth', false)} min={-2} max={2} step={0.05} onchange={set('depth', 0)} ondone={done} />
        <p class="note">Depth: how far it follows the cursor.</p>
      </section>
    {/if}

    <section>
      <h3>Look</h3>
      <Field label="Visible" kind="flag" value={get('visible', true) !== false} {editor} link={link('visible', false)} onchange={set('visible', true)} />
      <Field label="Opacity" value={get('opacity', 1)} {editor} link={link('opacity')} onchange={set('opacity', 1)} ondone={done} />
      <Field label="Blend" kind="choice" choices={BLENDS} value={get('blend', 'normal')} {editor} link={link('blend', false)} onchange={set('blend', 'normal')} />
    </section>

    <section>
      <h3>Effects</h3>
      {#each layer.effects ?? [] as e, i (i)}
        {@const effect = effectOf(e.type)}
        <div class="effect">
          <div class="row">
            <input type="checkbox" title="On" checked={editor.value(e.visible, true) !== false} onchange={(ev) => edits(i, (x) => (x.visible = ev.currentTarget.checked ? undefined : false))} />
            <strong class="grow" title={effect?.about}>{effect?.label ?? e.type}</strong>
            <button class="ghost small" onclick={() => moveEffect(i, -1)} disabled={i === 0} aria-label="Earlier">↑</button>
            <button class="ghost small" onclick={() => moveEffect(i, 1)} disabled={i === layer.effects.length - 1} aria-label="Later">↓</button>
            <button
              class="ghost small danger"
              aria-label="Remove"
              onclick={() => editor.edit(layer.id, (l) => { l.effects.splice(i, 1); if (!l.effects.length && l.type !== 'adjustment') l.effects = undefined; })}>✕</button
            >
          </div>
          {#each effect?.settings ?? [] as s (s.key)}
            <Field
              label={s.label}
              kind={s.type === 'image' ? 'file' : s.type}
              value={editor.value(e[s.key], otherwise(s))}
              {editor}
              link={s.type === 'image' ? null : effectLink(i, s, e, effect)}
              min={s.min}
              max={s.max}
              step={s.step}
              unit={s.unit ?? ''}
              choices={s.choices ?? []}
              onchange={(v) => edits(i, (x) => editor.assign(x, s.key, v), s.key)}
              ondone={done}
              onpick={() => pickAsset('image', (path) => edits(i, (x) => (x[s.key] = path)))}
            />
          {/each}
        </div>
      {/each}
      <select value="" aria-label="Add an effect" onchange={(e) => { addEffect(e.currentTarget.value); e.currentTarget.value = ''; }}>
        <option value="">Add an effect…</option>
        {#each groups as [label, list] (label)}
          <optgroup {label}>
            {#each list as e (e.name)}<option value={e.name} title={e.about}>{e.label}</option>{/each}
          </optgroup>
        {/each}
      </select>
    </section>
  {:else}
    <h2>Wallpaper</h2>
    <section>
      <Field label="Size" kind="choice" value={sizeId} choices={SIZES.some(([id]) => id === sizeId) ? SIZES : [[sizeId, editor.size.join(' × ')], ...SIZES]} onchange={(v) => wall('size', v.split('x').map(Number))} />
      <Field label="Background" kind="color" value={editor.value(editor.doc.background, '#000000')} {editor} link={wallLink('background')} onchange={(v) => wall('background', v)} ondone={done} />
      <p class="note">The background shows where no layer covers it.</p>
    </section>
    <section>
      <h3>Parallax</h3>
      <Field label="Amount" value={editor.value(parallax?.amount, parallax ? 0.05 : 0)} {editor} link={parallax ? { key: 'amount', moves: false, raw: parallax.amount, set: (raw) => editor.change((doc) => (doc.parallax.amount = made(raw))) } : null} max={0.2} step={0.005} onchange={(v) => setParallax('amount', v)} ondone={done} />
      <Field label="Smoothing" value={editor.value(parallax?.smoothing, 0.15)} max={2} step={0.05} unit="s" onchange={(v) => setParallax('smoothing', v)} ondone={done} />
      <p class="note">Layers with a depth follow the cursor.</p>
    </section>
    <ViewerSettings {editor} />
    <section>
      <h3>In the library</h3>
      <textarea rows="3" maxlength="5000" aria-label="Description" placeholder="Description" value={editor.project.description ?? ''} onchange={(e) => editor.changeProject((p) => (p.description = e.currentTarget.value))}></textarea>
      <input
        type="text"
        placeholder="Tags, separated by commas"
        aria-label="Tags"
        value={(editor.project.tags ?? []).join(', ')}
        onchange={(e) => editor.changeProject((p) => (p.tags = e.currentTarget.value.split(',').map((t) => t.trim()).filter(Boolean)))}
      />
    </section>
  {/if}

  {#if editor.problems.length}
    <section class="problems">
      <h3>Problems</h3>
      {#each editor.problems as p (p)}<p>{p}</p>{/each}
    </section>
  {/if}
</aside>

<style>
  aside {
    width: 320px;
    padding: 12px 14px 24px;
    background: var(--panel);
    border-left: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  h2 {
    flex: none;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .words {
    display: flex;
    align-items: flex-start;
    gap: 4px;
  }

  .words textarea {
    flex: 1;
    min-width: 0;
  }

  textarea {
    resize: vertical;
    font: inherit;
  }

  .effect {
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .grow {
    flex: 1;
    min-width: 0;
  }

  .small {
    padding: 0 6px;
  }

  .note {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
  }

  .problems p {
    margin: 0;
    font-size: 12px;
    color: var(--warn);
    overflow-wrap: anywhere;
  }
</style>
