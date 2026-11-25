import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fromHsv, normal, toHsv } from './color.js';

test('colours go both ways', () => {
  for (const hex of ['#000000', '#ffffff', '#ff0000', '#00ff00', '#0000ff', '#52cbb7', '#0e102e', '#808080']) {
    assert.equal(fromHsv(toHsv(hex)), hex);
  }
  assert.deepEqual(toHsv('#00ff00'), { h: 120, s: 1, v: 1 });
  assert.equal(fromHsv({ h: 360, s: 1, v: 1 }), '#ff0000');
  assert.equal(fromHsv({ h: 200, s: 2, v: -1 }), '#000000');
});

test('typed colours', () => {
  assert.equal(normal('abc'), '#aabbcc');
  assert.equal(normal(' #A0B0C0 '), '#a0b0c0');
  assert.equal(normal('#11223344'), '#112233');
  for (const bad of ['', '#12', 'red', null, '#gggggg']) assert.equal(normal(bad), null);
});
