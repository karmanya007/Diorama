// Where layers are, in the wallpaper's own pixels from its middle (x right,
// y down), and what dragging their handles does. A layer's place is its
// `position`, `scale` (a number or a pair) and `rotation` (degrees,
// clockwise); its box is around that position, before scaling and turning.

const rad = (deg) => (deg * Math.PI) / 180;

export const pair = (v, d = 1) => (Array.isArray(v) ? [Number(v[0] ?? d), Number(v[1] ?? d)] : [Number(v ?? d), Number(v ?? d)]);

/** Rounds what a drag makes: whole pixels, thousandths of scale. */
export const round = (v, digits = 0) => {
  const k = 10 ** digits;
  return Math.round(v * k) / k || 0;
};

export function turn([x, y], degrees) {
  const [s, c] = [Math.sin(rad(degrees)), Math.cos(rad(degrees))];
  return [x * c - y * s, x * s + y * c];
}

/** From a layer's own coordinates to the wallpaper's. */
export function toWorld(place, [x, y]) {
  const [sx, sy] = place.scale;
  const [tx, ty] = turn([x * sx, y * sy], place.rotation);
  return [place.position[0] + tx, place.position[1] + ty];
}

/** And back. */
export function toLayer(place, [x, y]) {
  const [sx, sy] = place.scale;
  const [tx, ty] = turn([x - place.position[0], y - place.position[1]], -place.rotation);
  return [tx / (sx || 1e-6), ty / (sy || 1e-6)];
}

/** Corners of `box` (x, y: its middle; width, height): top left, top right,
 * bottom right, bottom left. */
export function corners(place, box) {
  const [w, h] = [box.width / 2, box.height / 2];
  return [
    [-w, -h],
    [w, -h],
    [w, h],
    [-w, h],
  ].map(([x, y]) => toWorld(place, [box.x + x, box.y + y]));
}

export function contains(place, box, point) {
  const [x, y] = toLayer(place, point);
  return Math.abs(x - box.x) <= box.width / 2 && Math.abs(y - box.y) <= box.height / 2;
}

/**
 * The place after dragging handle `[hx, hy]` (each -1, 0 or 1: which side of
 * the box) to `point`. The opposite side stays where it is, or the middle
 * with `fromMiddle`; corners keep the proportions unless `free`.
 */
export function resize(place, box, [hx, hy], point, { free = false, fromMiddle = false } = {}) {
  const at = (x, y) => [box.x + (x * box.width) / 2, box.y + (y * box.height) / 2];
  const fixed = fromMiddle ? at(0, 0) : at(-hx, -hy);
  const held = at(hx, hy);
  const anchor = toWorld(place, fixed);
  // How far the cursor is from the fixed point, along the layer's own axes.
  const [dx, dy] = turn([point[0] - anchor[0], point[1] - anchor[1]], -place.rotation);
  const span = [held[0] - fixed[0], held[1] - fixed[1]];
  let [sx, sy] = place.scale;
  const small = (v, old) => (Math.abs(v) < 0.001 ? 0.001 * Math.sign(old || 1) : v);
  if (hx && hy && !free) {
    // Along the diagonal as it was.
    const [ox, oy] = [span[0] * sx, span[1] * sy];
    const k = (dx * ox + dy * oy) / (ox * ox + oy * oy || 1);
    [sx, sy] = [small(sx * k, sx), small(sy * k, sy)];
  } else {
    if (hx) sx = small(dx / span[0], sx);
    if (hy) sy = small(dy / span[1], sy);
  }
  const scaled = { ...place, scale: [sx, sy] };
  const moved = toWorld(scaled, fixed);
  return {
    ...scaled,
    position: [place.position[0] + anchor[0] - moved[0], place.position[1] + anchor[1] - moved[1]],
  };
}

/** The rotation after dragging the turning handle (above the box) to `point`. */
export function rotate(place, box, point, step = 0) {
  const middle = toWorld(place, [box.x, box.y]);
  const up = place.scale[1] < 0 ? -90 : 90;
  let angle = (Math.atan2(point[1] - middle[1], point[0] - middle[0]) * 180) / Math.PI + up;
  if (step) angle = Math.round(angle / step) * step;
  angle = ((((angle + 180) % 360) + 360) % 360) - 180;
  // Around the middle of the box, also where that is not the position.
  const turned = { ...place, rotation: angle };
  const moved = toWorld(turned, [box.x, box.y]);
  return {
    ...turned,
    position: [place.position[0] + middle[0] - moved[0], place.position[1] + middle[1] - moved[1]],
  };
}

/**
 * Moves `place` by `[dx, dy]`, pulled to the wallpaper's middle and edges
 * when within `reach`; `guides` are the lines it was pulled to.
 */
export function move(place, box, [dx, dy], [width, height], reach) {
  const moved = { ...place, position: [place.position[0] + dx, place.position[1] + dy] };
  const guides = [];
  if (reach > 0) {
    const points = corners(moved, box);
    for (const axis of [0, 1]) {
      const values = points.map((p) => p[axis]);
      const [lo, hi] = [Math.min(...values), Math.max(...values)];
      const half = (axis ? height : width) / 2;
      const pulls = [
        [0, (lo + hi) / 2],
        [-half, lo],
        [half, hi],
        [-half, hi],
        [half, lo],
      ]
        .map(([line, at]) => ({ line, by: line - at }))
        .filter((p) => Math.abs(p.by) <= reach)
        .sort((a, b) => Math.abs(a.by) - Math.abs(b.by));
      if (pulls.length) {
        moved.position[axis] += pulls[0].by;
        guides.push({ axis, at: pulls[0].line });
      }
    }
  }
  return { ...moved, guides };
}

/** A frame size with `size`'s proportions for an area of `[w, h]` device
 * pixels, of at most `most` pixels. */
export function frameSize([width, height], [w, h], most = 1280 * 720) {
  const k = Math.min(w / width, h / height, Math.sqrt(most / (width * height)));
  const side = (v) => Math.max(2, Math.round((v * k) / 2) * 2);
  return [side(width), side(height)];
}
