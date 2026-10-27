// Calls into the Rust side: lumend requests (`ipc`, the same JSON the CLI
// sends) and the library scan.
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { open } from '@tauri-apps/plugin-dialog';

/** One lumend request, e.g. `{ cmd: 'status' }`. Throws on daemon errors. */
export async function call(request) {
  const response = await invoke('ipc', { request });
  if (response.result === 'error') {
    throw new Error(response.message);
  }
  return response;
}

export const status = () => call({ cmd: 'status' });
export const costs = async () => (await call({ cmd: 'costs' })).costs;
export const library = (folders) => invoke('library', { folders });
export const presets = () => invoke('presets');
export const autostart = () => invoke('autostart');
export const setAutostart = (on) => invoke('set_autostart', { on });

const KINDS = {
  image: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'svg', 'avif'],
  video: ['mp4', 'webm', 'ogv', 'mov', 'm4v'],
  audio: ['mp3', 'ogg', 'wav', 'flac', 'm4a', 'aac', 'opus'],
};

/** The system's open dialog: a file (of WE's `fileType`) or a folder; null if cancelled. */
export async function pick({ folder = false, fileType, title } = {}) {
  const exts = KINDS[fileType];
  const picked = await open({
    directory: folder,
    multiple: false,
    title,
    filters: !folder && exts ? [{ name: fileType, extensions: exts }] : undefined,
  });
  return typeof picked === 'string' ? picked : null;
}

/** A local file as an image URL the page may load. */
export const fileUrl = (path) => (path ? convertFileSrc(path) : '');

/** Show `path` on `outputs` (output ids; empty: every monitor), with
 * `layout` 'per-output' (each shows all of it) or 'span' (one across them). */
export function setWallpaper(path, outputs, fit, layout) {
  const request = { cmd: 'set', path };
  if (outputs?.length) request.outputs = outputs;
  if (fit) request.fit = fit;
  if (layout) request.layout = layout;
  return call(request);
}

/** Let the web wallpaper `path` use the network (or not) and show it again
 * on `outputs` as it is (`content`: its status). */
export function setNetwork(path, allow, outputs, content) {
  const request = { cmd: 'set', path, network: allow, fit: content.fit, layout: content.layout };
  if (outputs?.length) request.outputs = outputs;
  return call(request);
}

export async function properties(path) {
  return (await call({ cmd: 'properties', path })).values;
}

export const setProperties = (path, properties) =>
  call({ cmd: 'set-properties', path, properties });

export async function playlists() {
  return (await call({ cmd: 'playlists' })).playlists;
}

export const savePlaylist = (playlist) => call({ cmd: 'save-playlist', playlist });
export const deletePlaylist = (name) => call({ cmd: 'delete-playlist', name });

export function play(name, outputs) {
  const request = { cmd: 'play', name };
  if (outputs?.length) request.outputs = outputs;
  return call(request);
}

export function next(outputs) {
  const request = { cmd: 'next' };
  if (outputs?.length) request.outputs = outputs;
  return call(request);
}

export const setPolicy = (policy) => call({ cmd: 'set-policy', policy });
export const setFolders = (folders) => call({ cmd: 'set-folders', folders });
export const setVolume = (percent) => call({ cmd: 'volume', percent });
/** Convert a video for hardware decoding (progress in status.conversions). */
export const convert = (path) => call({ cmd: 'convert', path });
export const pause = (on) => call({ cmd: on ? 'pause' : 'resume' });

/** This program's version. */
export const version = () => getVersion();

/** A newer signed release on GitHub, or null. Only asked when the user does. */
export const checkUpdate = () => check();

/** Downloads and installs `update` (the installer stops lumend and this
 * window), reporting progress 0–1, then starts the new version. */
export async function installUpdate(update, progress) {
  let total = 0;
  let done = 0;
  await update.downloadAndInstall((e) => {
    if (e.event === 'Started') total = e.data.contentLength ?? 0;
    if (e.event === 'Progress') done += e.data.chunkLength;
    if (total) progress(Math.min(done / total, 1));
  });
  await relaunch();
}
