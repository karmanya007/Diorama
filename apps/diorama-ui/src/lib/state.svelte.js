// State shared by the views: dioramad's status, the library, the selection.
import * as api from './api.js';

export const app = $state({
  /** dioramad's status (null until the first answer). */
  status: null,
  /** Why dioramad cannot be reached, if it cannot. */
  offline: '',
  items: [],
  /** What showing each wallpaper has cost (dioramad measures it, P12). */
  costs: [],
  scanning: false,
  /** The library item shown in the details panel. */
  selected: null,
  /** Output ids the next "Show" targets (empty: every monitor). */
  target: [],
  toast: '',
});

let toastTimer;

/** A short message at the bottom of the window. */
export function toast(message) {
  app.toast = message;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (app.toast = ''), 4000);
}

/** Runs `fn`, reporting failures as a toast. Returns its result or undefined. */
export async function attempt(fn) {
  try {
    return await fn();
  } catch (e) {
    toast(String(e?.message ?? e));
    return undefined;
  }
}

export async function refreshStatus() {
  try {
    app.status = await api.status();
    app.offline = '';
  } catch (e) {
    app.offline = String(e?.message ?? e);
  }
}

export async function refreshCosts() {
  try {
    app.costs = await api.costs();
  } catch {
    // Older daemon or offline: nothing measured to show.
  }
}

export async function scan() {
  app.scanning = true;
  try {
    app.items = await api.library(app.status?.folders ?? []);
  } catch (e) {
    toast(String(e?.message ?? e));
  } finally {
    app.scanning = false;
  }
}

const norm = (p) => (p ?? '').replaceAll('\\', '/').replace(/\/+$/, '').toLowerCase();

/** The library item a daemon path belongs to (folder, or a file inside one). */
export function itemFor(path) {
  const p = norm(path);
  if (!p) return null;
  return (
    app.items.find((i) => norm(i.path) === p) ??
    app.items.find((i) => p.startsWith(norm(i.path) + '/')) ??
    null
  );
}

/** The measured cost of a library item, or null. */
export function costOf(item) {
  const p = norm(item.path);
  return app.costs.find((c) => norm(c.path) === p || norm(c.path).startsWith(p + '/')) ?? null;
}

/** 'light', 'moderate' or 'heavy': how much showing it loads the machine. */
export function weight(cost) {
  if (cost.gpu >= 30 || cost.cpu >= 10) return 'heavy';
  if (cost.gpu >= 10 || cost.cpu >= 4) return 'moderate';
  return 'light';
}

/** A monitor's short name. */
export function monitorName(o) {
  return o.name || o.device.replace(/^\\\\\.\\/, '');
}
