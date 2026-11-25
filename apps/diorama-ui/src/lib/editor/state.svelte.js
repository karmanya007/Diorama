// The wallpaper being edited: its document with undo, saving (what is not
// saved is kept as a draft), and its preview (a process that draws it, see src-tauri/src/editor.rs).
import * as api from '../api.js';
import { toast } from '../state.svelte.js';
import { fromHex } from '../props.js';
import { pair } from './geometry.js';

const DRAFT_AFTER = 800;
/** Changes of one kind within this time are undone together. */
const MERGE_WITHIN = 1000;
const MAX_UNDO = 200;
/** A preview that runs this long did not crash right away. */
const LASTS = 10000;

const copy = (v) => JSON.parse(JSON.stringify(v));

export class Editor {
  dir = '';
  doc = $state(null);
  project = $state(null);
  /** Effects there are, by name. */
  effects = [];
  /** Installed font families. */
  fonts = $state.raw([]);
  /** Id of the selected layer. */
  selected = $state(null);
  /** Whether there are changes that are not saved. */
  dirty = $state(false);
  saving = $state(false);
  /** The changes are from an earlier time. */
  restored = $state(false);
  /** Where layers are (preview.rs, `LayerBox`), by id. */
  boxes = $state.raw({});
  problems = $state.raw([]);
  /** Why there is no preview, if there is none. */
  failure = $state('');
  past = $state.raw([]);
  future = $state.raw([]);

  #lastKind = null;
  #lastAt = 0;
  /** The document and project.json as they are saved (JSON). */
  #saved = '';
  #draftTimer;
  /** Writes of files, one after the other. */
  #writes = Promise.resolve();
  #sending = false;
  #unsent = false;
  #size = null;
  /** Starts of the preview that did not last, and when the last one was. */
  #restarts = 0;
  #startedAt = 0;
  #closed = false;
  #unlisten = null;
  /** Set by the stage: draws a frame. */
  ondraw = null;

  async open(dir) {
    this.dir = dir;
    const opened = await api.editorOpen(dir);
    this.effects = await api.editorEffects();
    // Looking through the fonts takes a moment the first time.
    api.editorFonts().then((fonts) => (this.fonts = fonts), () => {});
    this.#take(opened);
    this.#saved = this.#whole();
    if (opened.draft) {
      this.#take(opened.draft);
      this.restored = this.dirty = this.#whole() !== this.#saved;
    }
    this.#unlisten = await api.onPreview((event) => this.#heard(event));
    await this.#start();
  }

  #take({ document, project }) {
    if (!Array.isArray(document.layers)) document.layers = [];
    document.layers = document.layers.filter((l) => l && typeof l === 'object');
    for (const layer of document.layers) if (typeof layer.id !== 'string') layer.id = this.#newId(document);
    this.doc = document;
    this.project = project;
  }

  get layers() {
    return this.doc?.layers ?? [];
  }

  get layer() {
    return this.layers.find((l) => l.id === this.selected) ?? null;
  }

  get size() {
    const [w, h] = pair(this.doc?.size, 1);
    return [Math.max(1, w || 1), Math.max(1, h || 1)];
  }

  #newId(doc = this.doc) {
    const used = new Set((doc.layers ?? []).map((l) => l.id));
    let n = used.size + 1;
    while (used.has(`layer-${n}`)) n++;
    return `layer-${n}`;
  }

  // ---- values -------------------------------------------------------------

  /** A value as it is now: `v`, or the default of the user setting it names. */
  value(v, otherwise) {
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      return this.project?.general?.properties?.[v.property]?.value ?? otherwise;
    }
    return v ?? otherwise;
  }

  /** The user setting a value comes from, if it does. */
  bound(v) {
    return v && typeof v === 'object' && !Array.isArray(v) ? (v.property ?? null) : null;
  }

  /** A layer's place, as numbers. */
  place(layer) {
    const position = pair(this.value(layer.position, [0, 0]), 0);
    const scale = pair(this.value(layer.scale, 1), 1);
    const rotation = Number(this.value(layer.rotation, 0)) || 0;
    return { position, scale, rotation };
  }

  // ---- changes --------------------------------------------------------------

  /** What undo brings back: the document and the user settings. */
  #snapshot() {
    return JSON.stringify([this.doc, this.project?.general?.properties ?? {}]);
  }

  /** Changes the document (or user settings' defaults, see `assign`);
   * changes of the same `kind` in a row are one to undo. */
  change(fn, kind = null) {
    const before = this.#snapshot();
    fn(this.doc);
    if (this.#snapshot() === before) return;
    const now = performance.now();
    if (!kind || kind !== this.#lastKind || now - this.#lastAt > MERGE_WITHIN) {
      this.past = [...this.past.slice(1 - MAX_UNDO), before];
    }
    this.#lastKind = kind;
    this.#lastAt = now;
    this.future = [];
    this.#changed();
  }

  /** The next change is undone on its own. */
  settle() {
    this.#lastKind = null;
  }

  /** Changes the layer `id`. */
  edit(id, fn, kind) {
    this.change((doc) => {
      const layer = doc.layers.find((l) => l.id === id);
      if (layer) fn(layer);
    }, kind && `${kind} ${id}`);
  }

  /** Sets `owner[key]` within a change. A value that comes from a user
   * setting stays one: its default changes. */
  assign(owner, key, v) {
    const setting = this.project?.general?.properties?.[this.bound(owner[key])];
    if (!setting) owner[key] = v;
    else setting.value = setting.type === 'color' && /^#/.test(v ?? '') ? fromHex(v) : (v ?? setting.value);
  }

  undo() {
    this.#swap('past', 'future');
  }

  redo() {
    this.#swap('future', 'past');
  }

  #swap(from, to) {
    if (!this[from].length) return;
    this[to] = [...this[to], this.#snapshot()];
    const [doc, properties] = JSON.parse(this[from].at(-1));
    this.doc = doc;
    if (this.project.general) this.project.general.properties = properties;
    this[from] = this[from].slice(0, -1);
    this.settle();
    if (!this.layer) this.selected = null;
    this.#changed();
  }

  /** Adds `layer` above the selected one (or on top) and selects it. */
  add(layer) {
    const id = this.#newId();
    this.change((doc) => {
      const i = doc.layers.findIndex((l) => l.id === this.selected);
      doc.layers.splice(i < 0 ? doc.layers.length : i + 1, 0, { id, ...layer });
    });
    this.selected = id;
    return id;
  }

  remove(id) {
    const i = this.layers.findIndex((l) => l.id === id);
    this.change((doc) => doc.layers.splice(i, i < 0 ? 0 : 1));
    if (this.selected === id) this.selected = this.layers[Math.max(0, i - 1)]?.id ?? null;
  }

  duplicate(id) {
    const layer = this.layers.find((l) => l.id === id);
    if (!layer) return;
    const twin = copy(layer);
    const [x, y] = this.place(layer).position;
    twin.id = this.#newId();
    twin.name = `${layer.name ?? 'Layer'} copy`;
    if (!this.bound(layer.position)) twin.position = [x + 20, y + 20];
    this.change((doc) => doc.layers.splice(doc.layers.findIndex((l) => l.id === id) + 1, 0, twin));
    this.selected = twin.id;
  }

  /** Moves layer `id` to `index` (0: at the back). */
  reorder(id, index) {
    this.change((doc) => {
      const i = doc.layers.findIndex((l) => l.id === id);
      if (i < 0) return;
      const [layer] = doc.layers.splice(i, 1);
      doc.layers.splice(Math.max(0, Math.min(index, doc.layers.length)), 0, layer);
    });
  }

  /** Copies a file in and adds it as a layer. */
  async addFile(path) {
    const ext = path.split('.').pop().toLowerCase();
    const video = api.FILES.video.includes(ext);
    if (!video && !api.FILES.image.includes(ext)) throw new Error(`Diorama cannot show .${ext} files in a wallpaper`);
    const source = await api.editorAddAsset(this.dir, path);
    const name = path.split(/[\\/]/).pop().replace(/\.[^.]+$/, '');
    const id = this.add({ name, type: video ? 'video' : 'image', source, position: [0, 0] });
    const natural = await (video ? videoSize : imageSize)(api.fileUrl(`${this.dir}/${source}`));
    if (natural) {
      // The first picture covers the wallpaper; later ones fit into it.
      const [w, h] = this.size;
      const fit = this.layers.length === 1 ? Math.max(w / natural[0], h / natural[1]) : Math.min(1, (w * 0.6) / natural[0], (h * 0.6) / natural[1]);
      if (fit !== 1) this.edit(id, (l) => (l.scale = Math.round(fit * 1000) / 1000));
    }
    return id;
  }

  changeProject(fn) {
    fn(this.project);
    this.#touched();
  }

  // ---- saving ---------------------------------------------------------------

  #whole() {
    return JSON.stringify([this.doc, this.project]);
  }

  #changed() {
    this.#touched();
    this.#send();
  }

  #touched() {
    this.dirty = this.#whole() !== this.#saved;
    if (!this.dirty) this.restored = false;
    clearTimeout(this.#draftTimer);
    this.#draftTimer = setTimeout(() => this.keep(), DRAFT_AFTER);
  }

  #write(fn) {
    const done = this.#writes.then(fn);
    this.#writes = done.catch(() => {});
    return done;
  }

  /** Keeps what is not saved for the next time. */
  keep() {
    clearTimeout(this.#draftTimer);
    return this.#write(async () => {
      const draft = this.dirty ? { document: $state.snapshot(this.doc), project: $state.snapshot(this.project) } : null;
      await api.editorDraft(this.dir, draft);
    }).catch((e) => toast(`Changes could not be kept: ${e?.message ?? e}`));
  }

  /** Writes the wallpaper, with a preview picture for the library; where it
   * shows, it changes. */
  async save() {
    clearTimeout(this.#draftTimer);
    this.saving = true;
    try {
      await this.#write(() => this.#store());
    } finally {
      this.saving = false;
    }
  }

  async #store() {
    const before = JSON.parse(this.#saved);
    if (!this.failure) {
      // Without anything drawn the library shows a placeholder.
      await api.editorPicture(this.dir).then(() => (this.project.preview = 'preview.png'), () => {});
    }
    const whole = this.#whole();
    const [document, project] = JSON.parse(whole);
    await api.editorSaveProject(this.dir, project);
    await api.editorSave(this.dir, document);
    this.#saved = whole;
    this.dirty = this.#whole() !== whole;
    this.restored = false;
    if (!this.dirty) await api.editorDraft(this.dir, null).catch(() => {});
    // What is chosen here counts, also where the viewer chose something.
    const was = before[1]?.general?.properties ?? {};
    const reset = Object.entries(project.general?.properties ?? {})
      .filter(([name, p]) => name in was && JSON.stringify(p?.value) !== JSON.stringify(was[name]?.value))
      .map(([name]) => name);
    await api.reload(this.dir, reset);
  }

  /** Goes back to what is saved. */
  discard() {
    const [document, project] = JSON.parse(this.#saved);
    this.#take({ document, project });
    [this.past, this.future] = [[], []];
    this.settle();
    if (!this.layer) this.selected = null;
    this.#changed();
  }

  async close() {
    this.#closed = true;
    await this.keep();
    this.#unlisten?.();
    await api.previewStop();
  }

  // ---- preview --------------------------------------------------------------

  async #start() {
    this.failure = '';
    this.#startedAt = performance.now();
    await api.previewStart(this.dir);
    if (this.#size) await this.#command({ size: this.#size });
    // The file may be older than what is edited.
    this.#send();
    this.#frames();
  }

  async restart() {
    this.#restarts = 0;
    await this.#start();
  }

  async #frames() {
    let serial = 0;
    while (!this.#closed) {
      let frame;
      try {
        frame = await api.previewFrame(serial);
      } catch (e) {
        frame = { ended: true, reason: String(e) };
      }
      if (this.#closed) return;
      if (frame?.ended) {
        // A crashed preview comes back, unless it keeps crashing.
        if (performance.now() - this.#startedAt > LASTS) this.#restarts = 0;
        if (this.#restarts++ < 2) {
          await new Promise((r) => setTimeout(r, 700));
          if (!this.#closed) await this.#start().catch((e) => (this.failure = String(e?.message ?? e)));
        } else if (!this.failure) {
          this.failure = 'The preview stopped.';
        }
        return;
      }
      if (frame) {
        serial = frame.serial;
        this.ondraw?.(frame);
      }
    }
  }

  /** The stage's frames from now on. */
  setFrameSize(size) {
    if (this.#size && size[0] === this.#size[0] && size[1] === this.#size[1]) return;
    this.#size = size;
    this.#command({ size });
  }

  pointer(point) {
    this.#command({ pointer: point });
  }

  #command(command) {
    return api.previewCommand(command).catch(() => {});
  }

  /** Sends the document; while one is on its way, only the newest follows. */
  async #send() {
    if (this.#sending) {
      this.#unsent = true;
      return;
    }
    this.#sending = true;
    do {
      this.#unsent = false;
      const document = $state.snapshot(this.doc);
      // Defaults of the user settings may not be written yet.
      const settings = Object.entries($state.snapshot(this.project?.general?.properties ?? {}));
      const values = Object.fromEntries(settings.filter(([, s]) => s?.value !== undefined).map(([name, s]) => [name, s.value]));
      await this.#command({ document, values });
      await new Promise((r) => requestAnimationFrame(r));
    } while (this.#unsent && !this.#closed);
    this.#sending = false;
  }

  #heard(event) {
    if (event.event === 'error') {
      this.failure = event.message;
      this.#restarts = 99;
    } else if (event.event === 'document') {
      if (event.error) {
        this.problems = [event.error];
        return;
      }
      this.problems = event.problems;
      this.boxes = Object.fromEntries(event.layers.map((b) => [b.id, b]));
    }
  }
}

function imageSize(url) {
  return new Promise((resolve) => {
    const image = new Image();
    image.onload = () => resolve([image.naturalWidth, image.naturalHeight]);
    image.onerror = () => resolve(null);
    image.src = url;
  });
}

function videoSize(url) {
  return new Promise((resolve) => {
    const video = document.createElement('video');
    video.preload = 'metadata';
    video.onloadedmetadata = () => resolve([video.videoWidth, video.videoHeight]);
    video.onerror = () => resolve(null);
    setTimeout(() => resolve(null), 3000);
    video.src = url;
  });
}
