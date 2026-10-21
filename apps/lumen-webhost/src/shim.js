// Wallpaper Engine web-wallpaper API for pages hosted by lumen-webhost.
// Written for Lumen from WE's public documentation of the API; the host talks
// to it only through `window.__lumen`.
//
//   __lumen.properties(object)   user properties (`{name: {type, value}}`)
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
	function call(name, arg) {
		const l = listener();
		if (l && typeof l[name] === 'function') {
			try { l[name](arg); } catch (e) { console.error(e); }
			return true;
		}
		return false;
	}

	// Properties go out after `load` (pages register their listener while
	// parsing), then again whenever they change.
	function deliver() {
		if (!state.loaded) { return; }
		if (state.properties) { call('applyUserProperties', state.properties); }
		if (state.general) { call('applyGeneralProperties', state.general); }
	}
	window.addEventListener('load', () => {
		state.loaded = true;
		deliver();
		if (state.paused) { pause(true); }
	});
	L.properties = (p) => { state.properties = p; if (state.loaded) { call('applyUserProperties', p); } };
	L.general = (g) => { state.general = g; if (state.loaded) { call('applyGeneralProperties', g); } };

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

	// Files picked for directory properties are not supported yet.
	window.wallpaperRequestRandomFileForProperty = function (name, cb) {
		if (typeof cb === 'function') { setTimeout(() => cb(name, null), 0); }
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
