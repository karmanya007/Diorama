// Lumen's SceneScript host objects (thisScene, layers, engine, input, …).
// Runs after WE's own helper classes (Vec2/Vec3/…, createScriptProperties),
// which are loaded from the user's Wallpaper Engine install.
//
// Rust talks to this file through `globalThis.__lumen`:
//   init(stateJson)            scene description, before any script loads
//   attach(index, namespace)   one per script module, after it evaluated
//   start()                    scriptProperties, init(), applyUserProperties()
//   updatable()                indices of scripts with update()
//   begin(clock, runtime, frametime, cursorX, cursorY, daytime)  inputs, timers
//   run(index)                 one script's update(); the host calls each
//                              separately so a runaway script only stops itself
//   disable(index)             after the host interrupted it
//   end() → changes JSON
'use strict';
(function () {
	const L = globalThis.__lumen = globalThis.__lumen || {};
	L.clock = Date.now();
	L.logs = [];

	// Deterministic, host-driven wall clock (`new Date()`, `Date.now()`).
	const RealDate = Date;
	class LumenDate extends RealDate {
		constructor(...args) {
			if (args.length === 0) super(L.clock); else super(...args);
		}
		static now() { return L.clock; }
	}
	globalThis.Date = LumenDate;

	const say = (level) => (...args) => {
		if (L.logs.length < 64) {
			L.logs.push(level + ': ' + args.map((a) => {
				try { return typeof a === 'string' ? a : JSON.stringify(a); } catch (e) { return String(a); }
			}).join(' '));
		}
	};
	globalThis.console = { log: say('log'), info: say('log'), warn: say('warn'), error: say('error'), debug: say('log') };

	let anims = [];
	let nodes = [], layers = [], scripts = [], dirtyNodes = new Set(), dirtyLayers = new Set();
	let effectChanges = [], constantChanges = [], timers = [], timerId = 1;
	const objects = [];

	const vec3 = (a) => new Vec3(a[0], a[1], a[2]);
	const arr3 = (v) => (v instanceof Vec3 || v instanceof Vec2 || (v && typeof v === 'object'))
		? [Number(v.x) || 0, Number(v.y) || 0, Number(v.z) || 0]
		: (typeof v === 'string' ? arr3(new Vec3(v)) : [Number(v) || 0, Number(v) || 0, Number(v) || 0]);

	class Effect {
		constructor(layer, index) { this.__l = layer; this.__e = index; }
		get name() { return layers[this.__l].effects[this.__e].name; }
		get visible() { return layers[this.__l].effects[this.__e].visible; }
		set visible(v) {
			v = !!v;
			const e = layers[this.__l].effects[this.__e];
			if (e.visible !== v) { e.visible = v; effectChanges.push([this.__l, this.__e, v]); }
		}
		setMaterialProperty() {}
		executeMaterialFunction() {}
		getMaterial() { return this; }
		getMaterialCount() { return 1; }
	}

	const noopAnimation = {
		frameCount: 1, rate: 1, duration: 0,
		getFrame() { return 0; }, setFrame() {}, play() {}, pause() {}, stop() {}, isPlaying() { return false; },
	};

	class Layer {
		constructor(i) { this.__n = i; }
		get __layer() { return nodes[this.__n].layer; }
		get name() { return nodes[this.__n].name; }
		get id() { return nodes[this.__n].id; }
		get debug() { return false; }
		get origin() { return vec3(nodes[this.__n].origin); }
		set origin(v) { nodes[this.__n].origin = arr3(v); dirtyNodes.add(this.__n); }
		get scale() { return vec3(nodes[this.__n].scale); }
		set scale(v) { nodes[this.__n].scale = arr3(v); dirtyNodes.add(this.__n); }
		// Angles are degrees in scripts, radians in the scene IR.
		get angles() { return vec3(nodes[this.__n].angles.map((a) => a * 180 / Math.PI)); }
		set angles(v) { nodes[this.__n].angles = arr3(v).map((a) => a * Math.PI / 180); dirtyNodes.add(this.__n); }
		get visible() { return nodes[this.__n].visible; }
		set visible(v) { nodes[this.__n].visible = !!v; dirtyNodes.add(this.__n); }
		get parallaxDepth() { return new Vec2(1, 1); }
		set parallaxDepth(v) {}
		get alpha() { const l = this.__layer; return l === null ? (this.__alpha ?? 1) : layers[l].alpha; }
		set alpha(v) {
			const l = this.__layer;
			if (l === null) { this.__alpha = Number(v); return; }
			layers[l].alpha = Number(v); dirtyLayers.add(l);
		}
		get color() { const l = this.__layer; return l === null ? new Vec3(1) : vec3(layers[l].color); }
		set color(v) {
			const l = this.__layer;
			if (l === null) return;
			layers[l].color = arr3(v); dirtyLayers.add(l);
		}
		get text() { const l = this.__layer; return l === null ? '' : (layers[l].text ?? ''); }
		set text(v) {
			const l = this.__layer;
			if (l === null) return;
			layers[l].text = String(v); dirtyLayers.add(l);
		}
		// Text style: readable, and writable without effect yet (no re-layout).
		get __style() {
			const l = this.__layer;
			if (l === null) return this.__ownStyle || (this.__ownStyle = {});
			return layers[l].style || (layers[l].style = {});
		}
		get pointsize() { return this.__style.pointsize ?? 0; }
		set pointsize(v) { this.__style.pointsize = Number(v); }
		get font() { return this.__style.font ?? ''; }
		set font(v) { this.__style.font = String(v); }
		get padding() { return this.__style.padding ?? 0; }
		set padding(v) { this.__style.padding = v; }
		get horizontalalign() { return this.__style.horizontalalign ?? 'center'; }
		set horizontalalign(v) { this.__style.horizontalalign = String(v); }
		get verticalalign() { return this.__style.verticalalign ?? 'center'; }
		set verticalalign(v) { this.__style.verticalalign = String(v); }
		get anchor() { return this.__style.anchor ?? 'none'; }
		set anchor(v) { this.__style.anchor = String(v); }
		get limitrows() { return !!this.__style.maxrows; }
		set limitrows(v) { if (!v) this.__style.maxrows = null; }
		get maxrows() { return this.__style.maxrows ?? 1; }
		set maxrows(v) { this.__style.maxrows = Number(v); }
		get limitwidth() { return !!this.__style.maxwidth; }
		set limitwidth(v) { if (!v) this.__style.maxwidth = null; }
		get maxwidth() { return this.__style.maxwidth ?? 0; }
		set maxwidth(v) { this.__style.maxwidth = Number(v); }
		get opaquebackground() { return !!this.__style.opaquebackground; }
		set opaquebackground(v) { this.__style.opaquebackground = !!v; }
		get backgroundcolor() { return this.__style.backgroundcolor ?? new Vec3(0); }
		set backgroundcolor(v) { this.__style.backgroundcolor = v; }
		get alignment() { return this.__alignment ?? 'center'; }
		set alignment(v) { this.__alignment = v; }
		get solid() { return this.__solid ?? true; }
		set solid(v) { this.__solid = !!v; }
		get perspective() { return !!this.__perspective; }
		set perspective(v) { this.__perspective = !!v; }
		setParent(parent) {
			nodes[this.__n].parent = parent ? parent.__n : null;
			dirtyNodes.add(this.__n);
		}
		get size() { const l = this.__layer; return l === null ? new Vec2(0) : new Vec2(layers[l].size[0], layers[l].size[1]); }
		getParent() { const p = nodes[this.__n].parent; return p === null ? null : objects[p]; }
		getChildren() { return objects.filter((o) => nodes[o.__n].parent === this.__n); }
		getTransformMatrix() { return Mat4.identity ? Mat4.identity() : null; }
		getTextureAnimation() { return noopAnimation; }
		// Keyframe animation of one of this object's values (first, or by name).
		getAnimation(name) {
			const a = anims.find((x) => x.node === this.__n && (name === undefined || x.name === name));
			return a ? a.control : noopAnimation;
		}
		getVideoTexture() { return noopAnimation; }
		getAnimationLayer() { return noopAnimation; }
		getAnimationLayerCount() { return 0; }
		getEffectCount() { const l = this.__layer; return l === null ? 0 : layers[l].effects.length; }
		getEffect(which) {
			const l = this.__layer;
			if (l === null) return null;
			const i = typeof which === 'number' ? which : layers[l].effects.findIndex((e) => e.name === which);
			return i >= 0 && i < layers[l].effects.length ? new Effect(l, i) : null;
		}
		// Particle systems and sounds: not rendered yet.
		play() {} pause() {} stop() {} isPlaying() { return false; } emit() {}
		get instance() { return this.__instance || (this.__instance = {}); }
	}

	const scene = {
		getLayer(which) {
			if (typeof which === 'number') return objects[which] || null;
			return objects.find((o) => nodes[o.__n].name === which) || null;
		},
		getLayerByID(id) { return objects.find((o) => nodes[o.__n].id === id) || null; },
		enumerateLayers() { return objects.slice(); },
		getLayerCount() { return objects.length; },
		getLayerIndex(layer) { return objects.indexOf(layer); },
		// Layers made at runtime exist for the script but are not drawn yet.
		createLayer(config) {
			const c = (config && typeof config === 'object') ? config : {};
			nodes.push({
				name: String(c.name ?? ''), id: -1, parent: null, layer: null, visible: true,
				origin: c.origin ? arr3(c.origin) : [0, 0, 0],
				scale: c.scale ? arr3(c.scale) : [1, 1, 1],
				angles: [0, 0, 0],
			});
			const o = new Layer(nodes.length - 1);
			objects.push(o);
			return o;
		},
		sortLayer() {}, destroyLayer() {},
		getCameraTransforms() { return { eye: new Vec3(0, 0, 0), center: new Vec3(0, 0, -1), up: new Vec3(0, 1, 0) }; },
	};

	const audio = (n) => ({ left: new Float32Array(n), right: new Float32Array(n), average: new Float32Array(n), resolution: n });
	const engine = {
		frametime: 0, runtime: 0, timeOfDay: 0,
		userProperties: {},
		canvasSize: new Vec2(1920, 1080),
		screenResolution: new Vec2(1920, 1080),
		AUDIO_RESOLUTION_16: 16, AUDIO_RESOLUTION_32: 32, AUDIO_RESOLUTION_64: 64,
		registerAudioBuffers: audio,
		registerAsset(path) { return path; },
		isDesktopDevice() { return true; }, isMobileDevice() { return false; }, isTabletDevice() { return false; },
		isWallpaper() { return true; },
		isPortrait() { return engine.canvasSize.y > engine.canvasSize.x; },
		isLandscape() { return engine.canvasSize.x >= engine.canvasSize.y; }, isScreensaver() { return false; }, isRunningInEditor() { return false; },
		openUserShortcut() {},
		setTimeout(fn, ms) { const id = timerId++; timers.push({ id, at: engine.runtime + (Number(ms) || 0) / 1000, fn, every: 0 }); return id; },
		setInterval(fn, ms) { const s = Math.max(Number(ms) || 0, 1) / 1000; const id = timerId++; timers.push({ id, at: engine.runtime + s, fn, every: s }); return id; },
		clearTimeout(id) { timers = timers.filter((t) => t.id !== id); },
		clearInterval(id) { timers = timers.filter((t) => t.id !== id); },
	};
	globalThis.setTimeout = engine.setTimeout;
	globalThis.setInterval = engine.setInterval;
	globalThis.clearTimeout = engine.clearTimeout;
	globalThis.clearInterval = engine.clearInterval;

	const input = { cursorWorldPosition: new Vec3(0), cursorScreenPosition: new Vec2(0), cursorLeftDown: false };

	const storage = new Map();
	const localStorage = {
		LOCATION_GLOBAL: 0, LOCATION_SCREEN: 1,
		get(key) { return storage.has(key) ? storage.get(key) : undefined; },
		set(key, value) { storage.set(key, value); },
		delete(key) { storage.delete(key); },
		clear() { storage.clear(); },
	};

	Object.assign(globalThis, { thisScene: scene, engine, input, localStorage, shared: {}, renderContext: {} });

	// Values handed to update(): vectors as Vec2/3/4, like WE.
	function toScript(v) {
		if (typeof v === 'string' && /^\s*-?[\d.]+(e-?\d+)?(\s+-?[\d.]+(e-?\d+)?){1,3}\s*$/i.test(v)) {
			const n = v.trim().split(/\s+/).length;
			return n === 2 ? new Vec2(v) : n === 3 ? new Vec3(v) : new Vec4(v);
		}
		if (Array.isArray(v)) {
			return v.length === 2 ? new Vec2(v[0], v[1]) : v.length === 3 ? new Vec3(v[0], v[1], v[2]) : v;
		}
		return v;
	}
	const numbers = (v) => typeof v === 'number' ? [v]
		: typeof v === 'boolean' ? [v ? 1 : 0]
			: v instanceof Vec4 ? [v.x, v.y, v.z, v.w]
				: v instanceof Vec3 ? [v.x, v.y, v.z]
					: v instanceof Vec2 ? [v.x, v.y] : null;

	function current(s) {
		const o = s.node === null ? null : objects[s.node];
		switch (s.kind) {
			case 'origin': return o.origin;
			case 'scale': return o.scale;
			case 'angles': return o.angles;
			case 'visible': return o.visible;
			case 'alpha': return layers[s.a].alpha;
			case 'color': return vec3(layers[s.a].color);
			case 'text': return layers[s.a].text ?? '';
			case 'effectVisible': return layers[s.a].effects[s.b].visible;
			default: return s.value;
		}
	}

	function assign(s, v) {
		const o = s.node === null ? null : objects[s.node];
		switch (s.kind) {
			case 'origin': o.origin = v; break;
			case 'scale': o.scale = v; break;
			case 'angles': o.angles = v; break;
			case 'visible': o.visible = v; break;
			case 'alpha': o.alpha = v; break;
			case 'color': o.color = v; break;
			case 'text': o.text = v; break;
			case 'effectVisible': new Effect(s.a, s.b).visible = v; break;
			case 'constant': {
				const n = numbers(v);
				if (n && JSON.stringify(n) !== JSON.stringify(s.last)) {
					s.last = n;
					constantChanges.push([s.a, s.b, s.c, s.uniform, n]);
				}
				s.value = v;
				break;
			}
			default: s.value = v;
		}
	}

	// Keyframe animations: channels c0, c1, … of {frame, value, front, back}
	// keys with bezier handles (offsets in frames / value units), played at
	// options.fps over options.length frames: single (hold the end), loop,
	// or mirror (ping-pong). `relative` adds the value to the original.
	function bezier(a, b, f) {
		const h = (k, side, sign) => (k[side] && k[side].enabled !== false)
			? [Number(k[side].x) || 0, Number(k[side].y) || 0] : [0, 0];
		const [fx, fy] = h(a, 'front'), [bx, by] = h(b, 'back');
		const x0 = a.frame, y0 = Number(a.value), x3 = b.frame, y3 = Number(b.value);
		const x1 = Math.min(x0 + Math.max(fx, 0), x3), y1 = y0 + fy;
		const x2 = Math.max(x3 + Math.min(bx, 0), x0), y2 = y3 + by;
		const at = (p0, p1, p2, p3, t) => {
			const u = 1 - t;
			return u * u * u * p0 + 3 * u * u * t * p1 + 3 * u * t * t * p2 + t * t * t * p3;
		};
		let lo = 0, hi = 1;
		for (let i = 0; i < 32; i++) {
			const mid = (lo + hi) / 2;
			if (at(x0, x1, x2, x3, mid) < f) lo = mid; else hi = mid;
		}
		return at(y0, y1, y2, y3, (lo + hi) / 2);
	}
	function channelAt(keys, f) {
		if (!Array.isArray(keys) || keys.length === 0) return null;
		if (f <= keys[0].frame) return Number(keys[0].value);
		for (let i = 0; i + 1 < keys.length; i++) {
			if (f <= keys[i + 1].frame) return bezier(keys[i], keys[i + 1], f);
		}
		return Number(keys[keys.length - 1].value);
	}
	class Animation {
		constructor(spec) {
			Object.assign(this, spec);
			const k = spec.keyframes || {};
			const o = k.options || {};
			this.channels = [];
			for (let i = 0; Array.isArray(k['c' + i]); i++) this.channels.push(k['c' + i]);
			this.fps = Number(o.fps) > 0 ? Number(o.fps) : 30;
			this.length = Number(o.length) > 0 ? Number(o.length) : 1;
			this.mode = o.mode || 'loop';
			this.name = o.name;
			this.relative = !!k.relative;
			this.base = numbers(toScript(spec.initial)) || [0];
			this.playing = !o.startpaused;
			this.startedAt = 0;
			this.held = 0;
			this.rate = 1;
			const self = this;
			this.control = {
				get frameCount() { return self.length; },
				get rate() { return self.rate; }, set rate(r) { self.rate = Number(r) || 0; },
				play() { if (!self.playing) { self.startedAt = engine.runtime - self.held; self.playing = true; } },
				pause() { if (self.playing) { self.held = self.time(); self.playing = false; } },
				stop() { self.playing = false; self.held = 0; },
				isPlaying() { return self.playing; },
				getFrame() { return Math.floor(self.frame(engine.runtime)); },
				setFrame(f) { self.held = Number(f) / self.fps; self.startedAt = engine.runtime - self.held; },
			};
		}
		time(runtime = engine.runtime) {
			return this.playing ? Math.max(0, runtime - this.startedAt) : this.held;
		}
		frame(runtime) {
			const L = this.length;
			let f = this.time(runtime) * this.fps * this.rate;
			if (this.mode === 'single') return Math.min(f, L);
			if (this.mode === 'mirror') { f %= 2 * L; return f <= L ? f : 2 * L - f; }
			return f % L;
		}
		apply(runtime) {
			const f = this.frame(runtime);
			if (this.mode === 'single' && this.playing && f >= this.length) {
				this.held = this.length / this.fps; this.playing = false;
			}
			const v = this.channels.map((c, i) => {
				const x = channelAt(c, f) ?? 0;
				return this.relative ? x + (this.base[i] ?? 0) : x;
			});
			if (v.length === 0) return;
			// Keys hold scene units; scripts (and assign) use degrees for angles.
			const out = this.kind === 'angles' ? v.map((a) => a * 180 / Math.PI) : v;
			const value = out.length === 1 ? (this.kind === 'visible' ? out[0] > 0.5 : out[0])
				: out.length === 2 ? new Vec2(out[0], out[1]) : new Vec3(out[0], out[1], out[2]);
			assign(this, value);
		}
	}

	function fail(s, where, e) {
		s.errors = (s.errors || 0) + 1;
		if (s.errors <= 3) {
			const at = e && e.stack ? ' | ' + String(e.stack).trim().split(/\r?\n/)[0] : '';
			console.error('script ' + s.index + ' ' + where + ': ' + String(e) + at);
		}
		if (s.errors >= 3) s.disabled = true;
	}

	L.init = function (json) {
		const state = JSON.parse(json);
		nodes = state.nodes;
		layers = state.layers;
		for (let i = 0; i < nodes.length; i++) objects.push(new Layer(i));
		// Scene settings scripts read (`thisScene.cameraparallax`, …); colours as Vec3.
		for (const [k, v] of Object.entries(state.general)) {
			scene[k] = Array.isArray(v) ? vec3(v) : v;
		}
		engine.canvasSize = new Vec2(state.canvas[0], state.canvas[1]);
		engine.screenResolution = new Vec2(state.canvas[0], state.canvas[1]);
		try { engine.userProperties = _Internal.convertUserProperties(state.userProperties); } catch (e) { engine.userProperties = {}; }
		scripts = state.scripts.map((s, index) => Object.assign(s, { index, value: toScript(s.initial) }));
		anims = (state.animations || []).map((a) => new Animation(a));
	};
	L.layerFor = (node) => node === null ? null : objects[node];
	L.attach = function (index, ns) { scripts[index].ns = ns; };

	L.start = function () {
		for (const s of scripts) {
			if (!s.ns) continue;
			try {
				if (s.ns.scriptProperties && s.properties) _Internal.updateScriptProperties(s.ns, s.properties);
			} catch (e) { fail(s, 'scriptProperties', e); }
			try {
				if (typeof s.ns.init === 'function') {
					const r = s.ns.init(toScript(current(s)));
					if (r !== undefined) assign(s, r);
				}
				if (typeof s.ns.applyUserProperties === 'function') s.ns.applyUserProperties(engine.userProperties);
			} catch (e) { fail(s, 'init', e); }
		}
	};

	L.updatable = () => scripts
		.filter((s) => !s.disabled && s.ns && typeof s.ns.update === 'function')
		.map((s) => s.index);
	L.disable = (i) => { scripts[i].disabled = true; console.error('script ' + i + ' stopped (time budget)'); };

	// Cursor events (cursorEnter/Leave/Move/Down/Up/Click) for the scripts
	// of objects under the cursor; the host hit-tests layer rectangles.
	let hovered = new Map(), pressed = new Set(), wasDown = false, lastCursor = '';
	function dispatch(node, handler, world, local) {
		for (const s of scripts) {
			if (s.node !== node || s.disabled || !s.ns || typeof s.ns[handler] !== 'function') continue;
			try {
				s.ns[handler]({ worldPosition: world, localPosition: new Vec3(local[0], local[1], 0) });
			} catch (e) { fail(s, handler, e); }
		}
	}
	// Events for every script (media integration …).
	function revive(v) {
		if (v && typeof v === 'object' && Array.isArray(v.vec3)) return vec3(v.vec3);
		if (v && typeof v === 'object') { for (const k in v) v[k] = revive(v[k]); }
		return v;
	}
	function broadcast(handler, event) {
		event = revive(event);
		for (const s of scripts) {
			if (s.disabled || !s.ns || typeof s.ns[handler] !== 'function') continue;
			try { s.ns[handler](event); } catch (e) { fail(s, handler, e); }
		}
	}
	function pointer(cx, cy, state) {
		const world = new Vec3(cx, cy, 0);
		const down = !!state.down;
		const hits = new Map((state.hits || []).map((h) => [h[0], [h[1], h[2]]]));
		const moved = lastCursor !== cx + ',' + cy;
		lastCursor = cx + ',' + cy;
		for (const [n, local] of hovered) if (!hits.has(n)) dispatch(n, 'cursorLeave', world, local);
		for (const [n, local] of hits) {
			if (!hovered.has(n)) dispatch(n, 'cursorEnter', world, local);
			else if (moved) dispatch(n, 'cursorMove', world, local);
		}
		if (down && !wasDown) {
			pressed = new Set(hits.keys());
			for (const [n, local] of hits) dispatch(n, 'cursorDown', world, local);
		} else if (!down && wasDown) {
			for (const [n, local] of hits) {
				dispatch(n, 'cursorUp', world, local);
				if (pressed.has(n)) dispatch(n, 'cursorClick', world, local);
			}
			pressed = new Set();
		}
		for (const [handler, event] of state.events || []) broadcast(handler, event);
		hovered = hits;
		wasDown = down;
		input.cursorLeftDown = down;
	}

	L.begin = function (clock, runtime, frametime, cx, cy, daytime, pointerJson) {
		L.clock = clock;
		engine.runtime = runtime;
		engine.frametime = frametime;
		engine.timeOfDay = daytime;
		input.cursorWorldPosition = new Vec3(cx, cy, 0);
		input.cursorScreenPosition = new Vec2(cx, cy);
		for (const a of anims) {
			try { a.apply(runtime); } catch (e) { console.error('animation: ' + e); }
		}
		pointer(cx, cy, JSON.parse(pointerJson || '{}'));
		const due = timers.filter((t) => t.at <= runtime);
		for (const t of due) {
			if (t.every > 0) t.at = runtime + t.every; else timers = timers.filter((x) => x !== t);
			try { t.fn(); } catch (e) { console.error('timer: ' + e); }
		}
	};

	L.run = function (i) {
		const s = scripts[i];
		if (s.disabled) return;
		try {
			const r = s.ns.update(toScript(current(s)));
			if (r !== undefined) assign(s, r);
		} catch (e) { fail(s, 'update', e); }
	};

	L.end = function () {
		const out = { nodes: {}, layers: {}, effects: effectChanges, constants: constantChanges, logs: L.logs };
		for (const i of dirtyNodes) {
			const n = nodes[i];
			out.nodes[i] = { origin: n.origin, scale: n.scale, angles: n.angles, visible: n.visible };
		}
		for (const i of dirtyLayers) {
			const l = layers[i];
			out.layers[i] = { alpha: l.alpha, color: l.color, text: l.text };
		}
		dirtyNodes = new Set(); dirtyLayers = new Set(); effectChanges = []; constantChanges = []; L.logs = [];
		return JSON.stringify(out);
	};
})();
