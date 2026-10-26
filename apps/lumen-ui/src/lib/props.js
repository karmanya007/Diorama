// Wallpaper Engine property definitions (`general.properties` of
// project.json) turned into form fields.

/** Labels WE's own UI gives its localisation keys. */
const KNOWN = {
  ui_browse_properties_scheme_color: 'Scheme color',
  ui_browse_properties_alignment: 'Alignment',
  ui_browse_properties_volume: 'Volume',
  ui_browse_properties_playback_rate: 'Playback rate',
};

/** Plain text from a label that may hold HTML or a localisation key. */
export function labelOf(name, def) {
  const raw = String(def?.text ?? '').trim();
  if (KNOWN[raw]) return KNOWN[raw];
  if (/^ui_[a-z0-9_]+$/.test(raw) || raw === '') {
    const base = (raw || name).replace(/^ui_(browse_properties_|editor_properties_)?/, '');
    const words = base.replaceAll('_', ' ').trim();
    return words.charAt(0).toUpperCase() + words.slice(1);
  }
  const doc = new DOMParser().parseFromString(raw, 'text/html');
  return (doc.body.textContent ?? '').replace(/\s+/g, ' ').trim();
}

/** Fields in the order WE shows them. */
export function fields(properties) {
  if (!properties || typeof properties !== 'object') return [];
  return Object.entries(properties)
    .map(([name, def]) => ({ name, def, type: String(def?.type ?? '').toLowerCase() }))
    .filter((f) => f.def && typeof f.def === 'object')
    .sort((a, b) => (a.def.order ?? a.def.index ?? 0) - (b.def.order ?? b.def.index ?? 0));
}

/** WE colours are "r g b" in 0..1; inputs want #rrggbb. */
export function toHex(value) {
  const parts = String(value ?? '0 0 0').trim().split(/\s+/).map(Number);
  const hex = (x) =>
    Math.round(Math.min(1, Math.max(0, Number.isFinite(x) ? x : 0)) * 255)
      .toString(16)
      .padStart(2, '0');
  return '#' + [0, 1, 2].map((i) => hex(parts[i])).join('');
}

export function fromHex(hex) {
  const n = parseInt(hex.slice(1), 16);
  const c = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((v) => +(v / 255).toFixed(4));
  return c.join(' ');
}

// WE `condition`s are small JavaScript expressions over the other
// properties (`showclock.value == true && style.value != 2`). The page's
// CSP forbids eval, so a tiny parser handles the subset they use.

function tokens(src) {
  const out = [];
  const re =
    /\s*(?:(\d+(?:\.\d+)?)|('(?:[^'\\]|\\.)*'|"(?:[^"\\]|\\.)*")|([A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*)|(===|!==|==|!=|<=|>=|&&|\|\||[<>!()]))/y;
  let m;
  let i = 0;
  while (i < src.length) {
    re.lastIndex = i;
    m = re.exec(src);
    if (!m || m[0].length === 0) {
      if (/^\s*$/.test(src.slice(i))) break;
      throw new Error('bad condition');
    }
    i = re.lastIndex;
    if (m[1] !== undefined) out.push({ t: 'num', v: Number(m[1]) });
    else if (m[2] !== undefined) out.push({ t: 'str', v: m[2].slice(1, -1) });
    else if (m[3] !== undefined) out.push({ t: 'id', v: m[3] });
    else out.push({ t: 'op', v: m[4] });
  }
  return out;
}

/** Whether `condition` holds for `values` (`{name: value}`); true if it cannot be read. */
export function holds(condition, values) {
  if (!condition || !String(condition).trim()) return true;
  try {
    const ts = tokens(String(condition));
    let p = 0;
    const peek = () => ts[p];
    const eat = (v) => (ts[p]?.v === v ? (p++, true) : false);
    const primary = () => {
      const t = ts[p++];
      if (!t) throw new Error('end');
      if (t.t === 'op' && t.v === '(') {
        const v = or();
        if (!eat(')')) throw new Error(')');
        return v;
      }
      if (t.t === 'op' && t.v === '!') return !primary();
      if (t.t === 'num' || t.t === 'str') return t.v;
      if (t.v === 'true') return true;
      if (t.v === 'false') return false;
      const [name, field] = t.v.split('.');
      return field === 'value' || field === undefined ? values[name] : undefined;
    };
    const compare = () => {
      let a = primary();
      for (;;) {
        const op = peek()?.v;
        if (!['==', '===', '!=', '!==', '<', '>', '<=', '>='].includes(op)) return a;
        p++;
        const b = primary();
        // WE stores combo values as strings or numbers interchangeably.
        const [x, y] = typeof a === typeof b ? [a, b] : [String(a), String(b)];
        a = {
          '==': x == y,
          '===': x === y,
          '!=': x != y,
          '!==': x !== y,
          '<': x < y,
          '>': x > y,
          '<=': x <= y,
          '>=': x >= y,
        }[op];
      }
    };
    const and = () => {
      let a = compare();
      while (eat('&&')) a = compare() && a;
      return a;
    };
    const or = () => {
      let a = and();
      while (eat('||')) a = and() || a;
      return a;
    };
    const result = or();
    return p === ts.length ? !!result : true;
  } catch {
    return true;
  }
}
