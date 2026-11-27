const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { runInNewContext } = require('node:vm');

const worker = {};
runInNewContext(readFileSync(join(__dirname, '../public/winrate.js'), 'utf8'), { self: worker });
function estimate(board, turn) {
  let result;
  worker.postMessage = rates => { result = Array.from(rates); };
  worker.onmessage({ data: { board, turn } });
  return result;
}

test('the side to move can win first, including overlines and broken fours', () => {
  const board = Array(225).fill(0);
  for (const cell of [64, 80, 112, 128, 144]) board[cell] = 1;
  for (const cell of [9, 10, 11, 12]) board[cell] = 2;
  assert.deepEqual(estimate(board, 1), [0, 100, 0]);
  assert.deepEqual(estimate(board, 2), [0, 0, 100]);
  board[12] = 0;
  board[96] = 2; // blocks the diagonal
  board[13] = 2; // leaves a single winning gap for white
  assert.deepEqual(estimate(board, 2), [0, 0, 100]);
});

test('two open ends cannot both be blocked', () => {
  const board = Array(225).fill(0);
  board.fill(2, 110, 114);
  assert.deepEqual(estimate(board, 1), [0, 0, 100]);
});

test('the last empty point draws without wrapping lines across the board edge', () => {
  const board = Array.from({ length: 225 }, (_, i) => (i % 15 + 2 * Math.floor(i / 15)) % 4 < 2 ? 1 : 2);
  board[224] = 0;
  assert.deepEqual(estimate(board, 1), [100, 0, 0]);
});

test('handicap and undo snapshots reproduce the same complete distribution', () => {
  const board = Array(225).fill(0);
  for (const cell of [112, 128, 126]) board[cell] = 1;
  const opening = estimate(board, 2);
  assert.equal(opening.reduce((a, b) => a + b), 100);
  board[127] = 2;
  estimate(board, 1);
  board[127] = 0;
  assert.deepEqual(estimate(board, 2), opening);
});
