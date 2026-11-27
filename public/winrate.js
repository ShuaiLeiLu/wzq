// One hundred complete playouts give whole percentages for black, white and draws.
const SIZE = 15;
const CELLS = SIZE * SIZE;
const RUNS = 100;
const WIN = 10000;
const weights = [0, 2, 14, 100, WIN, 0];
const lines = [];
const through = Array.from({ length: CELLS }, () => []);
// Prefer central openings when no tactical feature distinguishes the moves.
const centerBias = Array.from({ length: CELLS }, (_, cell) =>
  SIZE - Math.abs(Math.floor(cell / SIZE) - 7) - Math.abs(cell % SIZE - 7));

for (let row = 0; row < SIZE; row++) {
  for (let col = 0; col < SIZE; col++) {
    for (const [dr, dc] of [[0, 1], [1, 0], [1, 1], [1, -1]]) {
      const endRow = row + 4 * dr, endCol = col + 4 * dc;
      if (endRow >= SIZE || endCol < 0 || endCol >= SIZE) continue;
      const line = Array.from({ length: 5 }, (_, n) => (row + n * dr) * SIZE + col + n * dc);
      const id = lines.length;
      lines.push(line);
      for (const cell of line) through[cell].push(id);
    }
  }
}

// Scores count unblocked five-cell lines through each empty point. A single
// four outweighs every possible combination of shorter lines through that point.
function place(board, cell, color, counts, scores) {
  board[cell] = color;
  const other = 3 - color;
  for (const id of through[cell]) {
    const own = counts[color][id]++;
    const theirs = counts[other][id];
    const attack = theirs ? 0 : weights[own + 1] - weights[own];
    const block = own ? 0 : weights[theirs];
    if (!attack && !block) continue;
    for (const point of lines[id]) {
      scores[color][point] += attack;
      scores[other][point] -= block;
    }
  }
}

function estimate(board, turn) {
  const counts = [null, new Uint8Array(lines.length), new Uint8Array(lines.length)];
  const scores = [null, new Int32Array(CELLS), new Int32Array(CELLS)];
  const initial = new Uint8Array(CELLS);
  let empty = CELLS;
  for (let cell = 0; cell < CELLS; cell++) {
    if (!board[cell]) continue;
    place(initial, cell, board[cell], counts, scores);
    empty--;
  }

  // The same snapshot must give both seats and spectators the same estimate.
  let seed = 1729;
  function random() {
    seed = (Math.imul(1664525, seed) + 1013904223) >>> 0;
    return seed / 4294967296;
  }
  const results = [0, 0, 0]; // draw, black, white
  for (let run = 0; run < RUNS; run++) {
    const stones = initial.slice();
    const lineCounts = [null, counts[1].slice(), counts[2].slice()];
    const moveScores = [null, scores[1].slice(), scores[2].slice()];
    let color = turn, winner = 0;
    for (let left = empty; left > 0; left--) {
      let best = -1, bestScore = -1, mustBlock = -1;
      for (let cell = 0; cell < CELLS; cell++) {
        if (stones[cell]) continue;
        const attack = moveScores[color][cell], defense = moveScores[3 - color][cell];
        if (attack >= WIN) { winner = color; break; }
        if (defense >= WIN) mustBlock = cell;
        const score = (centerBias[cell] + attack + 0.85 * defense) * (0.25 + random());
        if (score > bestScore) { best = cell; bestScore = score; }
      }
      if (winner) break;
      place(stones, mustBlock >= 0 ? mustBlock : best, color, lineCounts, moveScores);
      color = 3 - color;
    }
    results[winner]++;
  }
  return results;
}

self.onmessage = ({ data }) => self.postMessage(estimate(data.board, data.turn));
