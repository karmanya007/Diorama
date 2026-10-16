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
		get size() { const l = this.__layer; return l === null ? new Vec2(0) : new Vec2(layers[l].size[0], layers[l].size[1]); }
		getParent() { const p = nodes[this.__n].parent; return p === null ? null : objects[p]; }
		getChildren() { return objects.filter((o) => nodes[o.__n].parent === this.__n); }
		getTransformMatrix() { return Mat4.identity ? Mat4.identity() : null; }
		getTextureAnimation() { return noopAnimation; }
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
		isWallpaper() { return true; }, isScreensaver() { return false; }, isRunningInEditor() { return false; },
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

	L.begin = function (clock, runtime, frametime, cx, cy, daytime) {
		L.clock = clock;
		engine.runtime = runtime;
		engine.frametime = frametime;
		engine.timeOfDay = daytime;
		input.cursorWorldPosition = new Vec3(cx, cy, 0);
		input.cursorScreenPosition = new Vec2(cx, cy);
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
