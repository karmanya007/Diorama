// The wallpaper being edited: its document with undo, saving by itself, and
// its preview (a process that draws it, see src-tauri/src/editor.rs).
import * as api from '../api.js';
import { toast } from '../state.svelte.js';
import { pair } from './geometry.js';

const SAVE_AFTER = 800;
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
  /** 'saved', 'saving' or 'failed'. */
  status = $state('saved');
  /** Where layers are (preview.rs, `LayerBox`), by id. */
  boxes = $state.raw({});
  problems = $state.raw([]);
  /** Why there is no preview, if there is none. */
  failure = $state('');
  past = $state.raw([]);
  future = $state.raw([]);

  #lastKind = null;
  #lastAt = 0;
  #saveTimer;
  #saving = Promise.resolve();
  #projectDirty = false;
  #documentDirty = false;
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
    this.project = opened.project;
    const doc = opened.document;
    if (!Array.isArray(doc.layers)) doc.layers = [];
    doc.layers = doc.layers.filter((l) => l && typeof l === 'object');
    for (const layer of doc.layers) if (typeof layer.id !== 'string') layer.id = this.#newId(doc);
    this.doc = doc;
    this.#unlisten = await api.onPreview((event) => this.#heard(event));
    await this.#start();
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

  /** Changes the document; changes of the same `kind` in a row are one to undo. */
  change(fn, kind = null) {
    const before = JSON.stringify(this.doc);
    fn(this.doc);
    if (JSON.stringify(this.doc) === before) return;
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

  undo() {
    this.#swap('past', 'future');
  }

  redo() {
    this.#swap('future', 'past');
  }

  #swap(from, to) {
    if (!this[from].length) return;
    this[to] = [...this[to], JSON.stringify(this.doc)];
    this.doc = JSON.parse(this[from].at(-1));
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
    this.#projectDirty = true;
    this.#schedule();
  }

  // ---- saving ---------------------------------------------------------------

  #changed() {
    this.#documentDirty = true;
    this.#schedule();
    this.#send();
  }

  #schedule() {
    this.status = 'saving';
    clearTimeout(this.#saveTimer);
    this.#saveTimer = setTimeout(() => this.save(), SAVE_AFTER);
  }

  /** Writes what is not written yet. */
  save() {
    clearTimeout(this.#saveTimer);
    this.#saving = this.#saving.then(async () => {
      const [document, project] = [this.#documentDirty, this.#projectDirty];
      if (!document && !project) return;
      [this.#documentDirty, this.#projectDirty] = [false, false];
      try {
        if (document) await api.editorSave(this.dir, $state.snapshot(this.doc));
        if (project) await api.editorSaveProject(this.dir, $state.snapshot(this.project));
        if (!this.#documentDirty && !this.#projectDirty) this.status = 'saved';
      } catch (e) {
        this.#documentDirty ||= document;
        this.#projectDirty ||= project;
        this.status = 'failed';
        toast(`Not saved: ${e?.message ?? e}`);
      }
    });
    return this.#saving;
  }

  /** Saves everything, with a preview picture for the library. */
  async finish() {
    await this.save();
    if (this.failure) return;
    try {
      await api.editorPicture(this.dir);
      // It may have named the picture in project.json.
      this.project = { ...this.project, preview: 'preview.png' };
    } catch {
      // Nothing drawn: the library shows a placeholder.
    }
  }

  async close() {
    this.#closed = true;
    await this.finish();
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
      await this.#command({ document });
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
