// Colours as `#rrggbb` and as hue (0–360), saturation and value (0–1).

const clamp = (v, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, v));

/** `#rgb`, `#rrggbb` or `#rrggbbaa` as `#rrggbb`; null if it is none. */
export function normal(text) {
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6}|[0-9a-f]{8})$/i.exec(String(text ?? '').trim());
  if (!m) return null;
  const hex = m[1].toLowerCase();
  return `#${hex.length === 3 ? [...hex].map((c) => c + c).join('') : hex.slice(0, 6)}`;
}

export function toHsv(hex) {
  const n = parseInt((normal(hex) ?? '#000000').slice(1), 16);
  const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255].map((c) => c / 255);
  const max = Math.max(r, g, b);
  const d = max - Math.min(r, g, b);
  let h = 0;
  if (d) {
    if (max === r) h = ((g - b) / d + 6) % 6;
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
  }
  return { h: h * 60, s: max ? d / max : 0, v: max };
}

export function fromHsv({ h, s, v }) {
  const f = (n) => {
    const k = (n + h / 60) % 6;
    return clamp(v) - clamp(v) * clamp(s) * Math.max(0, Math.min(k, 4 - k, 1));
  };
  const hex = (c) =>
    Math.round(c * 255)
      .toString(16)
      .padStart(2, '0');
  return `#${hex(f(5))}${hex(f(3))}${hex(f(1))}`;
}
