// Calls into the Rust side: dioramad requests (`ipc`, the same JSON the CLI
// sends) and the library scan.
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { open, save } from '@tauri-apps/plugin-dialog';

/** One dioramad request, e.g. `{ cmd: 'status' }`. Throws on daemon errors. */
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

/** A colour the user picks from the screen; null if they do not. */
export const pickColor = () => invoke('pick_color');

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

export async function monitorPresets() {
  return (await call({ cmd: 'presets' })).presets;
}

/** Save what the monitors show as preset `number`; without `name` it keeps its own. */
export function savePreset(number, name) {
  const request = { cmd: 'save-preset', number };
  if (name !== undefined) request.name = name;
  return call(request);
}

export const renamePreset = (number, name) => call({ cmd: 'rename-preset', number, name });
export const deletePreset = (number) => call({ cmd: 'delete-preset', number });
export const usePreset = (number) => call({ cmd: 'use-preset', number });
/** `{ enabled, modifiers }`; what is left out stays. */
export const setHotkeys = (change) => call({ cmd: 'set-hotkeys', ...change });

export const setPolicy = (policy) => call({ cmd: 'set-policy', policy });
export const setFolders = (folders) => call({ cmd: 'set-folders', folders });
/** Lists `folder` in the library too. */
export async function addLibraryFolder(folder) {
  const { folders } = await status();
  if (!folders.includes(folder)) await setFolders([...folders, folder]);
}
export const setVolume = (percent) => call({ cmd: 'volume', percent });
/** Convert a video for hardware decoding (progress in status.conversions). */
export const convert = (path) => call({ cmd: 'convert', path });
export const pause = (on) => call({ cmd: on ? 'pause' : 'resume' });

/** This program's version. */
export const version = () => getVersion();

/** A newer signed release on GitHub, or null. Only asked when the user does. */
export const checkUpdate = () => check();

/** Downloads and installs `update` (the installer stops dioramad and this
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

// ---- the editor (src-tauri/src/editor.rs) ----

/** File types a wallpaper can hold. */
export const FILES = {
  image: ['png', 'jpg', 'jpeg', 'webp', 'bmp', 'tga', 'gif'],
  video: ['mp4', 'm4v', 'mov', 'webm', 'mkv', 'avi', 'wmv'],
  font: ['ttf', 'otf', 'ttc'],
};

/** The system's open dialog for files of `kind`; their paths. */
export async function pickFiles(kind, multiple = true) {
  const picked = await open({ multiple, filters: [{ name: kind, extensions: FILES[kind] }] });
  return picked === null ? [] : [picked].flat();
}

/** A `.diorama` file to open; null if cancelled. */
export async function pickPackage() {
  const picked = await open({ multiple: false, filters: [{ name: 'Diorama wallpaper', extensions: ['diorama'] }] });
  return typeof picked === 'string' ? picked : null;
}

/** Where to save a `.diorama` file; null if cancelled. */
export async function pickSave(name) {
  const file = await save({
    defaultPath: name.replace(/[\\/:*?"<>|]/g, '-'),
    filters: [{ name: 'Diorama wallpaper', extensions: ['diorama'] }],
  });
  return file && !file.toLowerCase().endsWith('.diorama') ? `${file}.diorama` : file;
}

/** Waits for `fn` before the window closes. */
export const onClose = (fn) => getCurrentWindow().onCloseRequested(fn);

/** Calls `fn` with the paths of files dropped on the window. */
export const onDrop = (fn) =>
  getCurrentWebview().onDragDropEvent((e) => {
    if (e.payload.type === 'drop') fn(e.payload.paths);
  });

export const editorCreate = (title, size) => invoke('editor_create', { title, size });
export const editorImport = (file) => invoke('editor_import', { file });
export const editorExport = (dir, file) => invoke('editor_export', { dir, file });
export const editorOpen = (dir) => invoke('editor_open', { dir });
export const editorSave = (dir, document) => invoke('editor_save', { dir, document });
export const editorSaveProject = (dir, project) => invoke('editor_save_project', { dir, project });
export const editorAddAsset = (dir, source) => invoke('editor_add_asset', { dir, source });
export const editorFonts = () => invoke('editor_fonts');
export const editorAddFont = (dir, family, bold, italic) => invoke('editor_add_font', { dir, family, bold, italic });
export const editorEffects = () => invoke('editor_effects');
export const editorPicture = (dir) => invoke('editor_picture', { dir });

export const previewStart = (dir) => invoke('preview_start', { dir });
export const previewCommand = (command) => invoke('preview_command', { command });
export const previewStop = () => invoke('preview_stop');
export const onPreview = (fn) => listen('preview', (e) => fn(e.payload));

/** The preview's next frame after `serial`: its pixels; null if there is
 * none yet, `{ ended: true }` if there will be none. */
export async function previewFrame(serial) {
  const response = await fetch(`${convertFileSrc('frame', 'preview')}?after=${serial}`);
  if (response.status === 204) return null;
  if (!response.ok) return { ended: true };
  const data = await response.arrayBuffer();
  const [n, width, height] = new Uint32Array(data, 0, 3);
  return { serial: n, width, height, pixels: new Uint8ClampedArray(data, 12, width * height * 4) };
}
