// Values that move: `{ keys: [{ t, v, ease }], length, repeat }` and
// `{ audio, from, to, smooth, threshold }` (docs/formats/diorama.md). What is
// shown here follows crates/diorama-scene/src/motion.rs; both are tested with
// the same cases.

export const EASES = [
  ['in-out', 'Smooth'],
  ['linear', 'Even'],
  ['in', 'Starts slowly'],
  ['out', 'Ends slowly'],
  ['hold', 'Stays, then jumps'],
];
export const REPEATS = [
  ['loop', 'Starts over'],
  ['mirror', 'There and back'],
  ['once', 'Once'],
];
export const RANGES = [
  ['bass', 'Bass'],
  ['middle', 'Middle'],
  ['treble', 'Treble'],
  ['all', 'Everything'],
];
/** Keys this close are at the same time. */
const CLOSE = 0.025;

const object = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);

/** Where a value comes from: 'keys', 'audio', 'setting', or null: it is fixed. */
export function sourceOf(v) {
  if (!object(v)) return null;
  if (typeof v.property === 'string') return 'setting';
  if (Array.isArray(v.keys)) return 'keys';
  return 'audio' in v ? 'audio' : null;
}

const time = (k) => Math.max(0, Number(k?.t) || 0);
export const sorted = (motion) => [...(motion.keys ?? [])].sort((a, b) => time(a) - time(b));

/** Seconds until it repeats or ends. */
export function lengthOf(motion) {
  const last = Math.max(0, ...(motion.keys ?? []).map(time));
  return Math.max(last, Number(motion.length) || 0);
}

const EASE = {
  linear: (u) => u,
  in: (u) => u * u,
  out: (u) => u * (2 - u),
  hold: () => 0,
  'in-out': (u) => u * u * (3 - 2 * u),
};

/** The value at `t` seconds; `read` makes values of what keys hold. */
export function valueAt(motion, t, read = (v) => v) {
  const keys = sorted(motion);
  if (!keys.length) return undefined;
  const [first, last] = [keys[0], keys.at(-1)];
  const length = lengthOf(motion);
  if (length <= 0) return read(last.v);
  const repeat = motion.repeat ?? 'loop';
  const rest = (a, b) => ((a % b) + b) % b;
  if (repeat === 'mirror') {
    t = rest(t, 2 * length);
    if (t > length) t = 2 * length - t;
  } else if (repeat === 'once') t = Math.min(length, Math.max(0, t));
  else t = rest(t, length);
  const next = keys.findIndex((k) => time(k) > t);
  let [from, to, start, end] = [];
  if (next > 0) [from, to, start, end] = [keys[next - 1], keys[next], time(keys[next - 1]), time(keys[next])];
  else if (repeat !== 'loop') return read(next === 0 ? first.v : last.v);
  else if (next === 0) [from, to, start, end] = [last, first, time(last) - length, time(first)];
  else [from, to, start, end] = [last, first, time(last), time(first) + length];
  if (end <= start) return read(from.v);
  const u = Math.min(1, Math.max(0, (t - start) / (end - start)));
  return mix(read(from.v), read(to.v), (EASE[from.ease] ?? EASE['in-out'])(u));
}

const hex = (v) => (typeof v === 'string' ? /^#([0-9a-f]{6})/i.exec(v)?.[1] : null);

/** Between two numbers, pairs or colours (`#rrggbb`). */
export function mix(a, b, k) {
  const [x, y] = [hex(a), hex(b)];
  if (x && y) {
    const part = (s, i) => parseInt(s.slice(2 * i, 2 * i + 2), 16);
    return `#${[0, 1, 2].map((i) => Math.round(part(x, i) + (part(y, i) - part(x, i)) * k).toString(16).padStart(2, '0')).join('')}`;
  }
  if (Array.isArray(a) || Array.isArray(b)) {
    const at = (v, i) => Number(Array.isArray(v) ? v[i] : v) || 0;
    const n = Math.max(Array.isArray(a) ? a.length : 0, Array.isArray(b) ? b.length : 0);
    return Array.from({ length: n }, (_, i) => at(a, i) + (at(b, i) - at(a, i)) * k);
  }
  const [m, n] = [Number(a), Number(b)];
  return Number.isFinite(m) && Number.isFinite(n) ? m + (n - m) * k : a;
}

const tidy = (t) => Math.round(Math.max(0, t) * 100) / 100;

/** The index (in `motion.keys`) of the key at `t`, or -1. */
export const keyAt = (motion, t) => (motion.keys ?? []).findIndex((k) => Math.abs(time(k) - t) <= CLOSE);

/** Puts `v` at `t`; the key's index. */
export function setKey(motion, t, v) {
  let i = keyAt(motion, t);
  if (i >= 0) {
    motion.keys[i].v = v;
    return i;
  }
  // It goes on as the one before it did.
  const before = sorted(motion).filter((k) => time(k) < t).at(-1);
  const key = { t: tidy(t), v };
  if (before?.ease) key.ease = before.ease;
  motion.keys.push(key);
  i = motion.keys.length - 1;
  if (motion.length !== undefined) motion.length = Math.max(motion.length, key.t);
  return i;
}

/** Moves key `i` to `t`, not onto another. */
export function moveKey(motion, i, t) {
  const to = tidy(t);
  if (motion.keys.some((k, j) => j !== i && Math.abs(time(k) - to) <= CLOSE)) return;
  motion.keys[i].t = to;
  if (motion.length !== undefined) motion.length = Math.max(motion.length, to);
}

/** The times of keys before and after `t`. */
export function around(motion, t) {
  const times = sorted(motion).map(time);
  return [times.filter((k) => k < t - CLOSE).at(-1) ?? null, times.find((k) => k > t + CLOSE) ?? null];
}

/** What can move of a layer, and what it is called. */
const PLACE = [['position', 'Position'], ['scale', 'Scale'], ['rotation', 'Rotation'], ['opacity', 'Opacity'], ['tint', 'Tint'], ['color', 'Colour']];

/** Every value of the document with keys. `effects`: those there are. */
export function tracks(doc, effects = []) {
  const found = [];
  const add = (layer, owner, key, label, effect = null) => {
    if (sourceOf(owner[key]) !== 'keys') return;
    const id = [layer.id, effect ?? '', key].join('\n');
    found.push({ id, layer, effect, key, label, motion: owner[key] });
  };
  for (const layer of doc?.layers ?? []) {
    for (const [key, label] of PLACE) add(layer, layer, key, label);
    (layer.effects ?? []).forEach((e, i) => {
      const effect = effects.find((x) => x.name === e?.type);
      for (const s of effect?.settings ?? []) add(layer, e, s.key, `${effect.label}: ${s.label}`, i);
    });
  }
  return found;
}

/** The seconds a timeline shows. */
export const spanOf = (list) => Math.max(5, Math.ceil(Math.max(0, ...list.map((t) => lengthOf(t.motion)))));

/** Something for a value to become with the loudest sound. */
export function louder(v, kind, key, { min = 0, max = 1 } = {}) {
  if (kind === 'color') return v;
  if (kind === 'pair') return [v[0], v[1] - 40];
  const n = Number(v) || 0;
  if (key === 'scale') return Math.round(n * 1.15 * 1000) / 1000;
  if (key === 'rotation') return n + 10;
  const step = (max - min) / 4;
  const to = n + step > max ? n - step : n + step;
  return Math.round(Math.min(max, Math.max(min, to)) * 1000) / 1000;
}
