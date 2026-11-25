import { test } from 'node:test';
import assert from 'node:assert/strict';
import { contains, corners, frameSize, move, pair, resize, rotate, toLayer, toWorld } from './geometry.js';

const near = (a, b, what = '') => a.forEach((v, i) => assert.ok(Math.abs(v - b[i]) < 1e-6, `${what} ${a} is not ${b}`));
const box = { x: 0, y: 0, width: 100, height: 50 };
const place = (o = {}) => ({ position: [10, 20], scale: [2, 1], rotation: 0, ...o });

test('points go both ways', () => {
  for (const p of [place(), place({ rotation: 33 }), place({ scale: [-2, 0.5], rotation: -120 })]) {
    near(toLayer(p, toWorld(p, [12, -7])), [12, -7]);
  }
  // A quarter turn clockwise: right becomes down.
  near(toWorld(place({ rotation: 90 }), [10, 0]), [10, 40]);
});

test('corners and hits', () => {
  near(corners(place(), box).flat(), [-90, -5, 110, -5, 110, 45, -90, 45]);
  assert.ok(contains(place(), box, [109, 44]));
  assert.ok(!contains(place(), box, [111, 44]));
  const turned = place({ rotation: 90 });
  assert.ok(contains(turned, box, [10, 119]) && !contains(turned, box, [100, 20]));
  // A text's box is beside its position.
  assert.ok(contains(place({ scale: [1, 1] }), { ...box, x: 50 }, [100, 20]));
});

test('a corner drag keeps the opposite corner and the proportions', () => {
  const before = place({ rotation: 25 });
  const fixed = corners(before, box)[0];
  const after = resize(before, box, [1, 1], toWorld(before, [75, 37.5]));
  near(corners(after, box)[0], fixed, 'fixed corner');
  assert.ok(Math.abs(after.scale[0] / after.scale[1] - 2) < 1e-9);
  near(after.scale, [2.5, 1.25]);
});

test('a side drag changes one direction, free corners both', () => {
  const side = resize(place(), box, [1, 0], [210, 999]);
  near([...side.scale, ...side.position], [3, 1, 60, 20]);
  const free = resize(place(), box, [1, 1], [210, 95], { free: true });
  near(free.scale, [3, 2]);
  near(corners(free, box)[0], [-90, -5]);
  // Past the fixed side it mirrors, and never becomes nothing.
  assert.ok(resize(place(), box, [1, 0], [-190, 0]).scale[0] < 0);
  assert.ok(resize(place(), box, [1, 0], [-90, 0]).scale[0] !== 0);
});

test('dragging from the middle keeps it', () => {
  const after = resize(place(), box, [1, 1], [210, 70], { fromMiddle: true });
  near(after.position, [10, 20]);
  near(after.scale, [4, 2]);
});

test('turning follows the cursor around the middle', () => {
  near([rotate(place(), box, [10, -100]).rotation], [0]);
  near([rotate(place(), box, [200, 20]).rotation], [90]);
  near([rotate(place(), box, [-200, 20]).rotation], [-90]);
  near([rotate(place(), box, [200, 10], 15).rotation], [90]);
  const text = { ...box, x: 50 };
  const before = toWorld(place(), [50, 0]);
  near(toWorld(rotate(place(), text, [300, 300]), [50, 0]), before, 'middle');
});

test('moves are pulled to the middle and the edges', () => {
  const small = { x: 0, y: 0, width: 10, height: 10 };
  const p = { position: [3, 0], scale: [1, 1], rotation: 0 };
  const middle = move(p, small, [1, 200], [1000, 600], 8);
  near(middle.position, [0, 200]);
  assert.deepEqual(middle.guides, [{ axis: 0, at: 0 }]);
  const edge = move(p, small, [489, 40], [1000, 600], 8);
  near(edge.position, [495, 40]);
  assert.deepEqual(edge.guides, [{ axis: 0, at: 500 }]);
  near(move(p, small, [1, 2], [1000, 600], 0).position, [4, 2]);
});

test('frames keep the proportions and stay small', () => {
  assert.deepEqual(frameSize([3840, 2160], [4000, 4000]), [1280, 720]);
  assert.deepEqual(frameSize([3840, 2160], [640, 900]), [640, 360]);
  const [w, h] = frameSize([1080, 1920], [4000, 4000]);
  assert.ok(w * h <= 1280 * 720 * 1.01 && Math.abs(w / h - 1080 / 1920) < 0.01);
  assert.deepEqual(frameSize([16384, 1], [10, 10])[1], 2);
});

test('scales are numbers or pairs', () => {
  assert.deepEqual(pair(undefined), [1, 1]);
  assert.deepEqual(pair(2), [2, 2]);
  assert.deepEqual(pair([1, 3]), [1, 3]);
});
