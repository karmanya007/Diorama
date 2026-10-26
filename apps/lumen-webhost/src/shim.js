// Wallpaper Engine web-wallpaper API for pages hosted by lumen-webhost.
// Written for Lumen from WE's public documentation of the API; the host talks
// to it only through `window.__lumen`.
//
//   __lumen.properties(object)   user properties (`{name: {type, value}}`)
//   __lumen.directories(object)  directory properties (`{name: {mode, files}}`)
//   __lumen.general(object)      general properties (`{fps}`)
//   __lumen.paused(bool)         pause / resume the page
//   __lumen.audio(array)         128 levels: 64 left then 64 right, linear (WE: mostly < 0.1)
//   __lumen.media(kind, object)  status | properties | thumbnail | playback | timeline
//   __lumen.pointer(x, y, kind)  cursor in CSS pixels: move | down | up
'use strict';
(function () {
	if (window.__lumen) { return; }
	const L = {};
	Object.defineProperty(window, '__lumen', { value: L });

	const state = {
		loaded: false,
		properties: null,
		directories: {},
		general: null,
		paused: false,
		audio: [],
		media: { status: [], properties: [], thumbnail: [], playback: [], timeline: [] },
		last: {},
	};

	function listener() {
		const l = window.wallpaperPropertyListener;
		return l && typeof l === 'object' ? l : null;
	}
	function call(name, ...args) {
		const l = listener();
		if (l && typeof l[name] === 'function') {
			try { l[name](...args); } catch (e) { console.error(e); }
			return true;
		}
		return false;
	}

	// Scripts and stylesheets that fail to load, and uncaught errors, go to
	// the host's log: a page missing a library often just stops silently.
	function report(msg) {
		try { window.ipc.postMessage(JSON.stringify(msg)); } catch (e) { /* no host */ }
	}
	window.addEventListener('error', (e) => {
		const t = e.target;
		if (t && t !== window && (t.tagName === 'SCRIPT' || t.tagName === 'LINK')) {
			report({ kind: 'load-failed', tag: t.tagName.toLowerCase(), url: String(t.src || t.href || '') });
		} else if (e.message) {
			report({ kind: 'error', message: String(e.message), source: String(e.filename || ''), line: e.lineno || 0 });
		}
	}, true);

	// Properties go out after `load` (pages register their listener while
	// parsing), then again whenever they change.
	function deliver() {
		if (!state.loaded) { return; }
		if (state.properties) { call('applyUserProperties', state.properties); }
		if (state.general) { call('applyGeneralProperties', state.general); }
		directoryChanges({}, state.directories);
	}
	window.addEventListener('load', () => {
		state.loaded = true;
		deliver();
		if (state.paused) { pause(true); }
	});
	L.properties = (p) => {
		state.properties = p;
		watchLocalFiles();
		adjust(p || {});
		if (state.loaded) { call('applyUserProperties', p); }
	};
	L.general = (g) => { state.general = g; if (state.loaded) { call('applyGeneralProperties', g); } };

	// WE's own options for every web wallpaper, stored with the properties:
	// a colour filter (`wec_e` on/off; contrast, brightness, saturation as
	// value/50; hue (value/50 - 1) * 180 degrees) and a horizontal flip.
	// Keys left out keep their last value, like WE.
	const filter = { on: false, con: 1, brs: 1, sa: 1, hue: 0 };
	let flip = null;
	function adjust(p) {
		const num = (k) => (p[k] && typeof p[k].value === 'number' ? p[k].value : null);
		if (p.wec_e && typeof p.wec_e.value === 'boolean') { filter.on = p.wec_e.value; }
		for (const k of ['con', 'brs', 'sa']) {
			const v = num('wec_' + k);
			if (v !== null) { filter[k] = v / 50; }
		}
		const hue = num('wec_hue');
		if (hue !== null) { filter.hue = (hue / 50 - 1) * 180; }
		if (p.alignmentfliph && typeof p.alignmentfliph.value === 'boolean') { flip = p.alignmentfliph.value; }
		applyAdjustments();
	}
	// Written only when they change, so a page's own filter or transform
	// stays while the options are off.
	const written = { filter: '', flip: null };
	function applyAdjustments() {
		const root = document.documentElement;
		if (!root) { return; }
		const css = filter.on
			? `contrast(${filter.con}) brightness(${filter.brs}) saturate(${filter.sa}) hue-rotate(${filter.hue}deg)`
			: '';
		if (css !== written.filter) {
			root.style.filter = css;
			written.filter = css;
		}
		if (flip !== null && flip !== written.flip) {
			root.style.transform = flip ? 'scaleX(-1)' : '';
			if (flip) { root.style.overflowY = 'hidden'; }
			written.flip = flip;
		}
	}
	// The init script runs before the document exists.
	document.addEventListener('DOMContentLoaded', applyAdjustments);

	// Directory properties: `fetchall` ones tell the page which files came
	// and went; `ondemand` ones answer wallpaperRequestRandomFileForProperty.
	function directoryChanges(before, now) {
		for (const name of new Set([...Object.keys(before), ...Object.keys(now)])) {
			const was = (before[name] && before[name].files) || [];
			const is = (now[name] && now[name].files) || [];
			const removed = was.filter((f) => !is.includes(f));
			const added = is.filter((f) => !was.includes(f));
			if ((now[name] || before[name]).mode !== 'fetchall') { continue; }
			if (removed.length) { call('userDirectoryFilesRemoved', name, removed); }
			if (added.length) { call('userDirectoryFilesAddedOrChanged', name, added); }
		}
	}
	L.directories = (d) => {
		const before = state.directories;
		state.directories = d || {};
		watchLocalFiles();
		if (state.loaded) { directoryChanges(before, state.directories); }
	};
	window.wallpaperRequestRandomFileForProperty = function (name, cb) {
		if (typeof cb !== 'function') { return; }
		const files = (state.directories[name] && state.directories[name].files) || [];
		const file = files.length ? files[Math.floor(Math.random() * files.length)] : null;
		setTimeout(() => cb(name, file), 0);
	};

	// Pages show picked files as 'file:///' + value, which the browser
	// refuses from this origin; the host serves them under LOCAL instead.
	const LOCAL = '/__lumen_file__/';
	function localUrl(v) {
		if (typeof v !== 'string' || v.slice(0, 8).toLowerCase() !== 'file:///') { return v; }
		return LOCAL + encodeURIComponent(v.slice(8));
	}
	function localCss(v) {
		if (typeof v !== 'string' || !/file:\/\/\//i.test(v)) { return v; }
		return v.replace(/(url\(\s*)(['"]?)file:\/\/\/(.*?)\2(\s*\))/gi,
			(m, open, q, path, close) => open + q + LOCAL + encodeURIComponent(path) + q + close);
	}
	function wrapSetter(proto, name, fix) {
		const d = proto && Object.getOwnPropertyDescriptor(proto, name);
		if (!d || !d.set || !d.configurable) { return; }
		Object.defineProperty(proto, name, Object.assign({}, d, { set(v) { d.set.call(this, fix(v)); } }));
	}
	wrapSetter(HTMLImageElement.prototype, 'src', localUrl);
	wrapSetter(HTMLMediaElement.prototype, 'src', localUrl);
	wrapSetter(HTMLSourceElement.prototype, 'src', localUrl);
	wrapSetter(HTMLVideoElement.prototype, 'poster', localUrl);
	const nativeSetAttribute = Element.prototype.setAttribute;
	Element.prototype.setAttribute = function (name, value) {
		const n = String(name).toLowerCase();
		if (n === 'src' || n === 'poster' || n === 'href') { value = localUrl(value); }
		else if (n === 'style') { value = localCss(value); }
		return nativeSetAttribute.call(this, name, value);
	};
	const nativeSetProperty = CSSStyleDeclaration.prototype.setProperty;
	CSSStyleDeclaration.prototype.setProperty = function (name, value, priority) {
		return nativeSetProperty.call(this, name, localCss(value), priority);
	};
	// `el.style.backgroundImage = …` and inserted HTML have no setter to
	// wrap: once the user has picked a file, fix what lands in the document.
	const fileAttrs = '[style*="file:"],[src^="file:"],[poster^="file:"]';
	function fixElement(el) {
		for (const a of ['src', 'poster']) {
			const v = el.getAttribute(a);
			if (v && localUrl(v) !== v) { nativeSetAttribute.call(el, a, localUrl(v)); }
		}
		const css = el.getAttribute('style');
		if (css && localCss(css) !== css) { nativeSetAttribute.call(el, 'style', localCss(css)); }
	}
	let observer = null;
	function watchLocalFiles() {
		const picked = Object.values(state.properties || {}).some((p) => p && p.type === 'file' && p.value)
			|| Object.values(state.directories).some((d) => d.files && d.files.length);
		if (!picked || observer) { return; }
		observer = new MutationObserver((records) => {
			for (const r of records) {
				if (r.type === 'attributes') { fixElement(r.target); continue; }
				for (const n of r.addedNodes) {
					if (n.nodeType !== 1) { continue; }
					fixElement(n);
					n.querySelectorAll(fileAttrs).forEach(fixElement);
				}
			}
		});
		observer.observe(document, { subtree: true, childList: true, attributes: true, attributeFilter: ['style', 'src', 'poster'] });
		if (document.documentElement) { document.querySelectorAll(fileAttrs).forEach(fixElement); }
	}

	// Pausing: animation frames wait, CSS animations, media and audio
	// contexts stop; the page hears setPaused.
	const nativeRaf = window.requestAnimationFrame.bind(window);
	const nativeCancel = window.cancelAnimationFrame.bind(window);
	let queued = new Map();
	let nextId = 1e9;
	window.requestAnimationFrame = function (cb) {
		if (!state.paused) { return nativeRaf(cb); }
		const id = nextId++;
		queued.set(id, cb);
		return id;
	};
	window.cancelAnimationFrame = function (id) {
		if (queued.delete(id)) { return; }
		nativeCancel(id);
	};
	const contexts = [];
	for (const name of ['AudioContext', 'webkitAudioContext']) {
		const Native = window[name];
		if (typeof Native !== 'function') { continue; }
		window[name] = class extends Native {
			constructor(...args) { super(...args); contexts.push(this); }
		};
	}
	let pausedMedia = [];
	let style = null;
	function pause(on) {
		if (on) {
			if (!style) {
				style = document.createElement('style');
				style.textContent = '*,*::before,*::after{animation-play-state:paused!important}';
			}
			(document.head || document.documentElement).appendChild(style);
			pausedMedia = Array.from(document.querySelectorAll('video,audio')).filter((m) => !m.paused);
			pausedMedia.forEach((m) => m.pause());
			contexts.forEach((c) => { if (c.state === 'running') { c.suspend(); } });
		} else {
			if (style) { style.remove(); }
			pausedMedia.forEach((m) => { m.play().catch(() => {}); });
			pausedMedia = [];
			contexts.forEach((c) => { if (c.state === 'suspended') { c.resume(); } });
			const waiting = queued;
			queued = new Map();
			for (const cb of waiting.values()) { nativeRaf(cb); }
		}
	}
	L.paused = (p) => {
		p = !!p;
		if (p === state.paused) { return; }
		state.paused = p;
		if (state.loaded) { pause(p); }
		call('setPaused', p);
	};

	// Audio: one listener list, 128 values per call.
	window.wallpaperRegisterAudioListener = function (fn) {
		if (typeof fn === 'function') { state.audio = [fn]; } else { state.audio = []; }
	};
	// Only after `load`, like WE: pages set up what their listener uses in
	// `load` handlers (3343374776 spins forever if called before p5.js runs).
	L.audio = (levels) => {
		if (state.paused || !state.loaded) { return; }
		const a = Float32Array.from(levels);
		for (const fn of state.audio) {
			try { fn(Array.from(a)); } catch (e) { console.error(e); }
		}
	};

	// Media integration.
	window.wallpaperMediaIntegration = Object.freeze({
		PLAYBACK_STOPPED: 0,
		PLAYBACK_PLAYING: 1,
		PLAYBACK_PAUSED: 2,
	});
	const registrations = {
		status: 'wallpaperRegisterMediaStatusListener',
		properties: 'wallpaperRegisterMediaPropertiesListener',
		thumbnail: 'wallpaperRegisterMediaThumbnailListener',
		playback: 'wallpaperRegisterMediaPlaybackListener',
		timeline: 'wallpaperRegisterMediaTimelineListener',
	};
	for (const [kind, name] of Object.entries(registrations)) {
		window[name] = function (fn) {
			state.media[kind] = typeof fn === 'function' ? [fn] : [];
			// A late listener still hears the current state.
			if (typeof fn === 'function' && state.last[kind]) {
				try { fn(state.last[kind]); } catch (e) { console.error(e); }
			}
		};
	}
	L.media = (kind, event) => {
		state.last[kind] = event;
		for (const fn of state.media[kind] || []) {
			try { fn(event); } catch (e) { console.error(e); }
		}
	};

	// The wallpaper sits behind the desktop icons and receives no mouse
	// input; the host forwards the cursor as synthetic events.
	let pressed = null;
	L.pointer = (x, y, kind) => {
		const target = document.elementFromPoint(x, y) || document.documentElement;
		const init = { bubbles: true, cancelable: true, clientX: x, clientY: y, screenX: x, screenY: y, view: window, button: 0 };
		const fire = (type, el, extra) => el.dispatchEvent(new MouseEvent(type, Object.assign({}, init, extra)));
		if (kind === 'move') {
			fire('mousemove', target, { buttons: pressed ? 1 : 0 });
		} else if (kind === 'down') {
			pressed = target;
			fire('mousedown', target, { buttons: 1 });
		} else if (kind === 'up') {
			fire('mouseup', target, { buttons: 0 });
			if (pressed && (pressed === target || pressed.contains(target))) { fire('click', target, { buttons: 0, detail: 1 }); }
			pressed = null;
		}
	};
})();
