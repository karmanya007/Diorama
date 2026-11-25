import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { around, keyAt, lengthOf, louder, mix, moveKey, setKey, sourceOf, spanOf, tracks, valueAt } from './motion.js';

const cases = JSON.parse(readFileSync(new URL('../../../../../crates/diorama-project/tests/motion.json', import.meta.url)));

for (const { name, motion, at } of cases) {
  test(`keys: ${name}`, () => {
    for (const [t, v] of at) {
      const got = valueAt(motion, t);
      assert.ok(Math.abs(got - v) < 1e-3, `${got}, not ${v} at ${t}`);
    }
  });
}

test('pairs and colours', () => {
  assert.deepEqual(mix([0, 10], [10, 20], 0.5), [5, 15]);
  assert.deepEqual(mix(1, [3, 5], 0.5), [2, 3]);
  assert.equal(mix('#000000', '#ff8040', 0.5), '#804020');
  assert.equal(mix('a', 'b', 0.5), 'a');
  const motion = { keys: [{ t: 0, v: { property: 'p' }, ease: 'linear' }, { t: 2, v: 4 }], repeat: 'once' };
  assert.equal(valueAt(motion, 1, (v) => (typeof v === 'object' ? 2 : v)), 3);
  assert.equal(valueAt({ keys: [] }, 1), undefined);
});

test('keys are set, replaced and moved', () => {
  const motion = { keys: [{ t: 0, v: 1, ease: 'linear' }], length: 2 };
  assert.equal(setKey(motion, 3.001, 5), 1);
  assert.deepEqual(motion.keys[1], { t: 3, v: 5, ease: 'linear' });
  assert.equal(motion.length, 3);
  assert.equal(setKey(motion, 3.01, 6), 1);
  assert.equal(motion.keys.length, 2);
  moveKey(motion, 1, 0.01);
  assert.equal(motion.keys[1].t, 3, 'not onto another');
  moveKey(motion, 1, 4.256);
  assert.equal(motion.keys[1].t, 4.26);
  assert.equal(lengthOf(motion), 4.26);
  assert.deepEqual(around(motion, 1), [0, 4.26]);
  assert.deepEqual(around(motion, 0), [null, 4.26]);
  assert.equal(keyAt(motion, 4.25), 1);
  assert.equal(keyAt(motion, 2), -1);
});

test('sources and tracks', () => {
  assert.deepEqual([1, [1, 2], null, { property: 'a' }, { keys: [] }, { audio: 'bass' }, { x: 1 }].map(sourceOf), [null, null, null, 'setting', 'keys', 'audio', null]);
  const keyed = { keys: [{ t: 7.2, v: 1 }] };
  const doc = { layers: [{ id: 'a', position: keyed, opacity: 1, effects: [{ type: 'blur', radius: keyed }, { type: 'gone', x: keyed }] }, { id: 'b' }] };
  const list = tracks(doc, [{ name: 'blur', label: 'Blur', settings: [{ key: 'radius', label: 'Radius' }] }]);
  assert.deepEqual(list.map((t) => [t.layer.id, t.effect, t.key, t.label]), [['a', null, 'position', 'Position'], ['a', 0, 'radius', 'Blur: Radius']]);
  assert.equal(spanOf(list), 8);
  assert.equal(spanOf([]), 5);
});

test('what a value becomes with sound stays in its range', () => {
  assert.equal(louder(1, 'number', 'opacity'), 0.75);
  assert.equal(louder(0.2, 'number', 'opacity'), 0.45);
  assert.deepEqual(louder([3, 4], 'pair', 'position'), [3, -36]);
  assert.equal(louder(2, 'number', 'scale'), 2.3);
  assert.equal(louder('#123456', 'color', 'tint'), '#123456');
});
