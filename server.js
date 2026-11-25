import { createServer } from 'node:http';
import { randomBytes, randomInt } from 'node:crypto';
import { createReadStream } from 'node:fs';

const size = 15;
const disconnectGrace = 45_000;
const spectatorLimit = 30;
const guidanceLimit = 40;
const players = new Map();
const rooms = new Map();
const staticFiles = new Map([
  ['/', ['index.html', 'text/html; charset=utf-8']],
  ['/app.js', ['app.js', 'text/javascript; charset=utf-8']],
  ['/style.css', ['style.css', 'text/css; charset=utf-8']],
  ['/chessroom.webp', ['chessroom.webp', 'image/webp']]
]);

function addRoom(id, name, theme, permanent = false) {
  const room = {
    id, name, theme, permanent, seats: [null, null], game: null,
    spectators: new Set(), guidance: [], nextMessageId: 1
  };
  rooms.set(id, room);
  return room;
}

addRoom('1001', '松风棋室', 'pine', true);
addRoom('1002', '听雨棋室', 'rain', true);
addRoom('1003', '竹影棋室', 'bamboo', true);
addRoom('1004', '青石棋室', 'stone', true);

function playerCount(room) {
  return room.seats.filter(Boolean).length;
}

function canJoin(room) {
  return room.game?.status !== 'playing' && playerCount(room) < 2;
}

function canWatch(room) {
  return room.spectators.size < spectatorLimit && (playerCount(room) > 0 || room.spectators.size > 0);
}

function snapshot(player) {
  const room = rooms.get(player.roomId);
  return {
    me: { name: player.name },
    online: [...players.values()].filter(item => item.stream).length,
    rooms: [...rooms.values()].map(item => ({
      id: item.id,
      name: item.name,
      theme: item.theme,
      count: playerCount(item),
      joinable: canJoin(item),
      watchable: canWatch(item),
      playing: item.game?.status === 'playing'
    })),
    room: room ? {
      id: room.id,
      name: room.name,
      seat: room.seats.indexOf(player),
      seats: room.seats.map(seat => seat ? {
        name: seat.name, ready: seat.ready, online: Boolean(seat.stream)
      } : null),
      game: room.game,
      spectatorCount: [...room.spectators].filter(spectator => spectator.stream).length,
      guidance: room.guidance
    } : null
  };
}

function publish(roomId) {
  for (const player of players.values()) {
    if (player.stream && (!roomId || player.roomId === roomId)) {
      player.stream.write(`data: ${JSON.stringify(snapshot(player))}\n\n`);
    }
  }
}

function leave(player) {
  const room = rooms.get(player.roomId);
  if (!room) return;
  const seat = room.seats.indexOf(player);
  if (seat === -1) {
    room.spectators.delete(player);
  } else {
    room.seats[seat] = null;
    if (room.game?.status === 'playing') {
      room.game.status = 'finished';
      room.game.winner = 2 - seat;
      room.game.reason = 'left';
    }
    for (const remaining of room.seats) if (remaining) remaining.ready = false;
  }
  player.roomId = null;
  player.ready = false;
  if (!playerCount(room)) room.game = null;
  if (!playerCount(room) && !room.spectators.size) {
    if (room.permanent) room.guidance.length = 0;
    else rooms.delete(room.id);
  }
}

function expireLater(player) {
  clearTimeout(player.expiry);
  player.expiry = setTimeout(() => {
    if (player.stream) return;
    leave(player);
    players.delete(player.token);
    publish();
  }, disconnectGrace);
  player.expiry.unref();
}

function beginIfReady(room) {
  if (room.game?.status === 'playing' || !room.seats.every(seat => seat?.stream && seat.ready)) return;
  room.game = {
    status: 'playing', board: Array(size * size).fill(0), turn: 1,
    moves: 0, last: null, line: [], winner: null, reason: null
  };
  for (const seat of room.seats) seat.ready = false;
}

function join(player, room) {
  if (!room) throw new Error('没有找到这个房间，请核对房间号。');
  if (room.seats.includes(player)) return;
  if (player.roomId && player.roomId !== room.id) throw new Error('请先退出当前房间。');
  if (!canJoin(room)) throw new Error('房间暂时无法加入，请换一个房间。');
  room.spectators.delete(player);
  room.seats[room.seats.indexOf(null)] = player;
  player.roomId = room.id;
  player.ready = false;
  room.game = null;
}

function watch(player, room) {
  if (!room) throw new Error('没有找到这个房间，请核对房间号。');
  if (room.spectators.has(player)) return;
  if (player.roomId) throw new Error('请先退出当前房间。');
  if (!canWatch(room)) throw new Error('这个房间当前无法观战。');
  room.spectators.add(player);
  player.roomId = room.id;
}

function winningLine(board, row, col, color) {
  for (const [dr, dc] of [[0, 1], [1, 0], [1, 1], [1, -1]]) {
    const line = [row * size + col];
    for (const direction of [-1, 1]) {
      let r = row + dr * direction;
      let c = col + dc * direction;
      while (r >= 0 && r < size && c >= 0 && c < size && board[r * size + c] === color) {
        if (direction === -1) line.unshift(r * size + c);
        else line.push(r * size + c);
        r += dr * direction;
        c += dc * direction;
      }
    }
    if (line.length >= 5) return line;
  }
  return [];
}

function move(player, room, index) {
  const game = room.game;
  if (game?.status !== 'playing') throw new Error('请等待双方准备后开局。');
  if (!room.seats.every(seat => seat?.stream)) throw new Error('棋友正在重连，请稍候。');
  const color = room.seats.indexOf(player) + 1;
  if (game.turn !== color) throw new Error('现在轮到对方落子。');
  if (!Number.isInteger(index) || index < 0 || index >= game.board.length || game.board[index] !== 0) {
    throw new Error('请选择棋盘上的空交叉点。');
  }
  game.board[index] = color;
  game.last = index;
  game.moves++;
  game.line = winningLine(game.board, Math.floor(index / size), index % size, color);
  if (game.line.length) {
    game.status = 'finished';
    game.winner = color;
    game.reason = 'line';
  } else if (game.moves === game.board.length) {
    game.status = 'finished';
    game.reason = 'draw';
  } else {
    game.turn = 3 - color;
  }
}

function cleanName(input, limit) {
  return typeof input === 'string' ? [...input.replace(/[\u0000-\u001f\u007f]/gu, '').trim()].slice(0, limit).join('') : '';
}

function sendGuidance(player, room, input) {
  if (!room?.spectators.has(player)) throw new Error('只有观战者可以发送指导。');
  const text = typeof input === 'string' ? input.replace(/[\u0000-\u001f\u007f]/gu, ' ').trim() : '';
  if (!text || [...text].length > 160) throw new Error('请输入 1 至 160 个字符。');
  const now = Date.now();
  if (now - (player.lastGuidanceAt ?? 0) < 1000) throw new Error('请稍等一秒再发送。');
  player.lastGuidanceAt = now;
  room.guidance.push({ id: room.nextMessageId++, name: player.name, text });
  if (room.guidance.length > guidanceLimit) room.guidance.shift();
  return room.id;
}

function command(player, data) {
  const room = rooms.get(player.roomId);
  switch (data.action) {
    case 'rename': {
      const name = cleanName(data.name, 16);
      if (!name) throw new Error('请输入昵称。');
      player.name = name;
      break;
    }
    case 'create': {
      if (room) throw new Error('请先退出当前房间。');
      if (rooms.size >= 100) throw new Error('房间数量已达上限，请稍后再试。');
      let id;
      do { id = String(randomInt(2000, 10000)); } while (rooms.has(id));
      join(player, addRoom(id, cleanName(data.name, 20) || `${player.name}的棋室`, 'pine'));
      break;
    }
    case 'join':
      join(player, rooms.get(String(data.id)));
      break;
    case 'watch':
      watch(player, rooms.get(String(data.id)));
      break;
    case 'guidance':
      return sendGuidance(player, room, data.text);
    case 'quick': {
      const available = [...rooms.values()].filter(canJoin);
      const target = available.find(item => playerCount(item) === 1) ?? available[0];
      if (!target) throw new Error('暂时没有可加入的房间，可以创建一间新棋室。');
      join(player, target);
      break;
    }
    case 'leave':
      leave(player);
      break;
    case 'ready':
      if (!room?.seats.includes(player) || room.game?.status === 'playing') throw new Error('当前无法切换准备状态。');
      player.ready = data.ready === true;
      beginIfReady(room);
      break;
    case 'move':
      if (!room?.seats.includes(player)) throw new Error('只有入座棋手可以落子。');
      move(player, room, data.index);
      break;
    default:
      throw new Error('无法执行此操作。');
  }
}

function send(res, status, data) {
  res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store' });
  res.end(JSON.stringify(data));
}

async function readBody(req) {
  if (!req.headers['content-type']?.startsWith('application/json')) throw new Error('请求格式无效。');
  let body = '';
  req.setEncoding('utf8');
  for await (const chunk of req) {
    body += chunk;
    if (body.length > 4096) throw new Error('请求内容过长。');
  }
  const json = JSON.parse(body);
  if (!json || typeof json !== 'object' || Array.isArray(json)) throw new Error('请求内容无效。');
  return json;
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url, 'http://localhost');
  if (url.pathname.startsWith('/api/') && req.headers['sec-fetch-site'] === 'cross-site') {
    return send(res, 403, { error: '请直接打开游戏页面。' });
  }

  if (req.method === 'GET' && url.pathname === '/healthz') return send(res, 200, { ok: true });

  if (req.method === 'GET' && url.pathname === '/api/events') {
    const player = players.get(url.searchParams.get('token'));
    res.writeHead(200, {
      'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache, no-transform',
      Connection: 'keep-alive', 'X-Accel-Buffering': 'no', 'Referrer-Policy': 'no-referrer'
    });
    if (!player) return res.end('event: expired\ndata: {}\n\n');
    const previous = player.stream;
    player.stream = res;
    clearTimeout(player.expiry);
    previous?.end();
    const room = rooms.get(player.roomId);
    if (room) beginIfReady(room);
    res.on('close', () => {
      if (player.stream !== res) return;
      player.stream = null;
      expireLater(player);
      publish();
    });
    publish();
    return;
  }

  if (req.method === 'POST' && url.pathname === '/api/session') {
    try {
      const data = await readBody(req);
      let player = players.get(data.token);
      if (!player) {
        const token = randomBytes(24).toString('hex');
        player = { token, name: `访客 ${randomInt(1000, 10000)}`, roomId: null, ready: false, stream: null };
        players.set(token, player);
        expireLater(player);
      }
      return send(res, 200, { token: player.token });
    } catch {
      return send(res, 400, { error: '无法创建棋手身份，请重试。' });
    }
  }

  if (req.method === 'POST' && url.pathname === '/api/action') {
    const player = players.get(req.headers.authorization?.slice(7));
    if (!player) return send(res, 401, { error: '连接已过期，请重试。' });
    try {
      const data = await readBody(req);
      if (!player.stream) return send(res, 409, { error: '正在重新连接棋室，请稍候。' });
      publish(command(player, data));
      return send(res, 200, { ok: true });
    } catch (error) {
      return send(res, 400, { error: error instanceof SyntaxError ? '请求内容无效。' : error.message });
    }
  }

  const file = staticFiles.get(url.pathname);
  if (file && req.method === 'GET') {
    res.writeHead(200, { 'Content-Type': file[1], 'Referrer-Policy': 'no-referrer', 'X-Content-Type-Options': 'nosniff' });
    createReadStream(new URL(`./public/${file[0]}`, import.meta.url)).pipe(res);
    return;
  }
  if (url.pathname === '/favicon.ico') return res.writeHead(204).end();
  send(res, 404, { error: '页面不存在。' });
});

setInterval(() => {
  for (const player of players.values()) player.stream?.write(': keep-alive\n\n');
}, 15_000).unref();

const port = Number(process.env.PORT || 3000);
server.listen(port, process.env.HOST || '0.0.0.0', () => {
  console.log(`弈间五子棋 · http://localhost:${server.address().port}`);
});
