// State shared by the views: lumend's status, the library, the selection.
import * as api from './api.js';

export const app = $state({
  /** lumend's status (null until the first answer). */
  status: null,
  /** Why lumend cannot be reached, if it cannot. */
  offline: '',
  items: [],
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

/** A monitor's short name. */
export function monitorName(o) {
  return o.name || o.device.replace(/^\\\\\.\\/, '');
}
