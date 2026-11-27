import { createDdz } from './ddz.js';
import { openHistory } from './history.js';

const $ = selector => document.querySelector(selector);
const PATHS = { hub: '/', gomoku: '/wzq', ddz: '/ddz' };
const GAME_NAMES = { gomoku: '五子棋', ddz: '斗地主' };

const TURN_MS = 30_000;
const SIZE = 15;
const COLUMNS = Array.from({ length: SIZE }, (_, index) => String.fromCharCode(65 + index));
const SESSION_KEY = 'yijian-session';
const BARRAGE_KEY = 'yijian-barrage';
const HANDICAP_LABELS = ['不让', '让先', '让二子', '让三子'];
const RULES = '自由五子棋 · 15 × 15 · 每手 30 秒';

const el = {
  lobby: $('#lobby'),
  roomView: $('#room-view'),
  roomList: $('#room-list'),
  search: $('#room-search'),
  joinCode: $('#join-code'),
  joinError: $('#join-error'),
  board: $('#board'),
  barrage: $('#barrage'),
  barrageToggle: $('#barrage-toggle'),
  chat: $('#chat'),
  chatLog: $('#chat-log'),
  chatInput: $('#chat-input'),
  ready: $('#ready'),
  takeSeat: $('#take-seat'),
  undo: $('#undo'),
  undoDialog: $('#undo-dialog'),
  upperSeat: $('#upper-seat'),
  lowerSeat: $('#lower-seat'),
  dialog: $('#dialog'),
  dialogTitle: $('#dialog-title'),
  dialogInput: $('#dialog-input'),
  dialogSubmit: $('#dialog-submit'),
  dialogError: $('#dialog-error'),
  auth: $('#auth-dialog'),
  authForm: $('#auth-form'),
  authEmail: $('#auth-email'),
  authPassword: $('#auth-password'),
  authCode: $('#auth-code'),
  authError: $('#auth-error'),
  authSubmit: $('#auth-submit'),
  ranking: $('#ranking-dialog'),
  rankingStatus: $('#ranking-status'),
  rankingTable: $('#ranking-table'),
  rankingOwn: $('#ranking-own'),
  rankingRetry: $('#ranking-retry'),
  podium: $('#podium'),
  result: $('#result-dialog'),
  navLobby: $('#nav-lobby'),
  hub: $('#hub'),
  ddzLobby: $('#ddz-lobby'),
  ddzView: $('#ddz-view')
};

let state = null;
let token = null;
let events = null;
let connected = false;
let syncedAt = 0;
let retryTimer;
let visibleRoom = null;
let renderedRooms = '';
let filter = 'all';
let boardFocus = 112;
let dialogAction = null;
let authBusy = false;
let authStep = 'credentials';
let toastTimer;
let lastGameKey = null;
let oddsKey = null;
let oddsWorker = null;
let sideRankingTimer;
let rankingTimer;
let rankingPending = false;
let rankingGame = 'gomoku';
let hubRankingTimer;
let view = viewFromPath();
let knownRoom = null;
let knownTable = null;
let visibleKey = null;
let atTable = false;
const pending = new Set();
const barrageQueue = [];
const points = [];

const linkParams = new URL(location.href).searchParams;
let requestedRoom = linkParams.get('room');
const linkedAction = linkParams.get('view') === 'watch' ? 'watch' : 'join';

/* ---------- Helpers ---------- */

function viewFromPath() {
  const path = location.pathname.replace(/\/+$/, '');
  return path === '/ddz' ? 'ddz' : path === '/wzq' ? 'gomoku' : 'hub';
}

function serverNow() {
  return state ? state.serverTime + performance.now() - syncedAt : 0;
}

function icon(name) {
  return `<svg class="icon" aria-hidden="true"><use href="#i-${name}"/></svg>`;
}

function escapeHTML(value) {
  const characters = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
  return String(value).replace(/[&<>"']/g, character => characters[character]);
}

function initial(name) {
  return Array.from(String(name || '弈').trim())[0] || '弈';
}

function tone(name) {
  let hash = 0;
  for (const character of String(name)) hash = (hash * 31 + character.codePointAt(0)) >>> 0;
  return `tone-${hash % 6}`;
}

function avatarInner(name, url, stone) {
  const face = url ? `<img src="${escapeHTML(url)}" alt="" referrerpolicy="no-referrer">` : escapeHTML(initial(name));
  return face + (stone ? `<span class="stone stone-${stone}"></span>` : '');
}

function scoreText(points) {
  return points > 0 ? `+${points}` : points < 0 ? `−${Math.abs(points)}` : '0';
}

function setText(node, text) {
  if (node.textContent !== text) node.textContent = text;
}

function setHTML(node, html) {
  if (node.dataset.html !== html) {
    node.innerHTML = html;
    node.dataset.html = html;
  }
}

function storage(value) {
  try {
    if (value === undefined) return sessionStorage.getItem(SESSION_KEY);
    if (value === null) sessionStorage.removeItem(SESSION_KEY);
    else sessionStorage.setItem(SESSION_KEY, value);
  } catch {
    // The game still works with an in-memory identity when storage is blocked.
  }
  return null;
}

function toast(message) {
  const node = $('#toast');
  node.textContent = message;
  node.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { node.hidden = true; }, 4200);
}

/* ---------- Network ---------- */

async function post(path, data, authenticated = false) {
  const headers = { 'Content-Type': 'application/json' };
  if (authenticated) headers.Authorization = `Bearer ${token}`;
  let response;
  try {
    response = await fetch(path, { method: 'POST', headers, body: JSON.stringify(data), signal: AbortSignal.timeout(10_000) });
  } catch {
    throw new Error('连接暂时不可用，请稍后重试。');
  }
  const result = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(result.error || '操作未完成，请重试。');
  return result;
}

async function run(action, data = {}, reportError = toast) {
  const group = action === 'guidance' ? 'message' : 'room';
  if (pending.has(group)) return false;
  if (!connected) {
    reportError('正在重新连接棋室，请稍候。');
    return false;
  }
  pending.add(group);
  try {
    await post('/api/action', { action, ...data }, true);
    return true;
  } catch (error) {
    reportError(error.message);
    return false;
  } finally {
    pending.delete(group);
  }
}

// Two tabs must not share one seat: a new tab checks whether the stored token is already live elsewhere.
const sibling = typeof BroadcastChannel === 'function' ? new BroadcastChannel('yijian-tabs') : null;
sibling?.addEventListener('message', event => {
  if (event.data.check && event.data.check === token) sibling.postMessage({ occupied: token });
});

function claimStoredIdentity(value) {
  if (!value || !sibling) return Promise.resolve(value);
  return new Promise(resolve => {
    let occupied = false;
    const hear = event => { if (event.data.occupied === value) occupied = true; };
    sibling.addEventListener('message', hear);
    sibling.postMessage({ check: value });
    setTimeout(() => {
      sibling.removeEventListener('message', hear);
      resolve(occupied ? null : value);
    }, 125);
  });
}

async function connect(identity) {
  clearTimeout(retryTimer);
  try {
    const session = await post('/api/session', { token: identity });
    token = session.token;
    storage(token);
    const source = new EventSource(`/api/events?token=${encodeURIComponent(token)}`);
    events = source;
    source.onmessage = event => {
      if (events !== source) return;
      connected = true;
      state = JSON.parse(event.data);
      syncedAt = performance.now();
      render();
      if (requestedRoom) {
        const id = requestedRoom;
        requestedRoom = null;
        if (!state.room) run(linkedAction, { id }).then(ok => { if (!ok) syncURL(null); });
      }
    };
    source.onerror = () => {
      if (events !== source) return;
      connected = false;
      render();
    };
    source.addEventListener('expired', () => {
      if (events !== source) return;
      source.close();
      events = null;
      connected = false;
      token = null;
      storage(null);
      render();
      connect(null);
    });
  } catch {
    setText($('#online span'), '正在重连');
    retryTimer = setTimeout(() => connect(identity), 1800);
  }
}

function syncURL(id, watching = false) {
  const url = new URL(location.href);
  url.pathname = PATHS[view];
  if (id) url.searchParams.set('room', id);
  else url.searchParams.delete('room');
  if (watching) url.searchParams.set('view', 'watch');
  else url.searchParams.delete('view');
  if (url.href !== location.href) history.replaceState(null, '', url);
}

/* ---------- Header ---------- */

function renderHeader() {
  const online = $('#online');
  online.classList.toggle('connected', connected);
  setText(online.querySelector('span'), connected ? `${state.online} 位在线` : state ? '正在重连' : '正在连接');
  for (const button of document.querySelectorAll('[data-network]')) button.disabled = !connected;

  const champions = [
    ['gomoku', state?.champion?.winner],
    ['ddz', state?.ddzChampion?.winner]
  ].filter(([game, winner]) => winner && (view === 'hub' || view === game));
  setHTML($('#champion .champion-inner'), champions.map(([game, winner]) => `
    <div class="champion-entry">
      <span class="champion-tag">${icon('crown')}昨日${GAME_NAMES[game]}榜首</span>
      <p>恭喜 <b>${escapeHTML(winner.name)}</b> 拿下昨日第一</p>
      <strong class="champion-score">${scoreText(winner.points)} 分</strong>
    </div>`).join(''));
  $('#champion').hidden = champions.length === 0;

  if (!state) return;
  const { me } = state;
  const account = me.account;
  const profile = $('[data-action="profile"]');
  profile.setAttribute('aria-label', account ? '账号与昵称' : '登录');
  setText($('#profile-name'), me.name);
  $('#profile-login').hidden = Boolean(account);
  $('#profile-edit').hidden = !account;
  const avatar = $('#profile-avatar');
  avatar.className = `avatar avatar-sm ${tone(me.name)}`;
  setHTML(avatar, avatarInner(me.name, account?.avatar));
  $('#account-name').hidden = !account;
  setText($('#account-name'), account ? `智算账号 · ${account.username}` : '');
  $('#account-login').hidden = Boolean(account);
  $('#account-logout').hidden = !account;
}

/* ---------- Lobby ---------- */

function roomThumb(room) {
  const stones = [];
  if (room.count >= 1) stones.push([7, 7, 'b']);
  if (room.count === 2) stones.push([8, 6, 'w']);
  if (room.playing) stones.push([8, 8, 'b'], [6, 8, 'w', 'last']);
  const position = value => `${(6.25 * (value + 1)).toFixed(3)}%`;
  const discs = stones.map(([x, y, color, last]) => `<i class="mini-stone ${color}${last ? ' last' : ''}" style="left:${position(x)};top:${position(y)}"></i>`).join('');
  return `<div class="room-thumb" aria-hidden="true"><div class="wood"><div class="wood-lines"></div>${discs}</div></div>`;
}

function roomCard(room) {
  const status = room.playing ? '对弈中' : !room.joinable ? '已满员' : room.count ? '等一位' : '空桌';
  const chipTone = room.playing ? 'live' : room.joinable && room.count ? 'wait' : '';
  const note = room.playing ? '对局进行中，可入场观战' : room.count === 2 ? '两位棋手正在准备' : room.count === 1 ? '一位棋手等待对手' : '虚位以待，入座即可开局';
  const seats = `<span class="seat-dots" aria-hidden="true">${[0, 1].map(index => `<span class="stone ${index < room.count ? 'stone-b' : 'stone-w'}" style="${index < room.count ? '' : 'opacity:.55'}"></span>`).join('')}</span>`;
  const disabled = connected ? '' : 'disabled';
  const watch = room.watchable ? `<button class="btn btn-line" type="button" data-action="watch" data-id="${room.id}" ${disabled}>${icon('eye')}观战</button>` : '';
  const join = room.joinable
    ? `<button class="btn btn-gold-flat" type="button" data-action="join" data-id="${room.id}" ${disabled}>入座</button>`
    : room.watchable ? '' : '<button class="btn btn-line" type="button" disabled>暂不可加入</button>';
  return `<article class="room-card${room.joinable ? ' open' : ''}">
    ${roomThumb(room)}
    <div class="room-main">
      <div class="room-title"><h3 title="${escapeHTML(room.name)}">${escapeHTML(room.name)}</h3><span class="room-no">#${room.id}</span><span class="chip ${chipTone}"><i></i>${status}</span></div>
      <div class="room-seats"><span>${seats}${room.count} / 2 入座</span><span class="vacant">${note}</span></div>
      <div class="room-foot"><span>15 × 15 · 每手 30 秒</span><div class="room-actions">${watch}${join}</div></div>
    </div>
  </article>`;
}

function renderLobby() {
  if (!state) return;
  const playing = state.rooms.filter(room => room.playing).length;
  setText($('#stat-online'), String(state.online));
  setText($('#stat-playing'), String(playing));
  setText($('#hero-chip-title'), playing ? `${playing} 局正在对弈` : '棋桌已备好');
  setText($('#hero-chip-note'), playing ? '点「观战」旁观高手过招' : '邀一位棋友，来一局');

  const query = el.search.value.trim().toLowerCase().replace(/^#/, '');
  const matches = state.rooms.filter(room => (filter === 'all' || (filter === 'open' ? room.joinable : room.watchable))
    && (room.name.toLowerCase().includes(query) || room.id.includes(query)));
  setText($('#room-count'), matches.length === state.rooms.length
    ? `${state.rooms.length} 间棋室 · ${playing} 局对弈中`
    : `显示 ${matches.length} / ${state.rooms.length} 间`);

  const signature = JSON.stringify([connected, matches]);
  if (signature === renderedRooms) return;
  renderedRooms = signature;
  const focused = el.roomList.contains(document.activeElement) ? { ...document.activeElement.dataset } : null;
  el.roomList.setAttribute('aria-busy', 'false');
  el.roomList.innerHTML = matches.length
    ? matches.map(roomCard).join('')
    : '<p class="empty">没有符合条件的棋室，换个筛选条件，或者自己开一间。</p>';
  if (focused?.id) {
    const same = el.roomList.querySelector(`[data-id="${focused.id}"][data-action="${focused.action}"]:not(:disabled)`);
    (same ?? el.roomList.querySelector(`[data-id="${focused.id}"]:not(:disabled)`))?.focus();
  }
}

async function fetchRanking() {
  const response = await fetch('/api/leaderboard', { signal: AbortSignal.timeout(7000) });
  if (!response.ok) throw new Error('Ranking unavailable');
  return response.json();
}

function ownRankText(ranking) {
  if (!state?.me?.account) return '登录后对局计入排行';
  return ranking.you ? `你今日第 <b>${ranking.you.rank}</b> 名 · ${scoreText(ranking.you.points)} 分` : '今天还没有计分对局';
}

async function refreshSideRanking(listId = '#side-rank-list', ownId = '#side-rank-own') {
  const list = $(listId);
  try {
    const ranking = await fetchRanking();
    const rows = ranking.entries.slice(0, 5).map(row => `<li class="${row.rank === 1 ? 'top' : ''}">
      <span class="medal ${row.rank <= 3 ? `medal-${row.rank}` : ''}">${row.rank}</span>
      <span class="avatar ${tone(row.name)}">${escapeHTML(initial(row.name))}</span>
      <span class="rank-who"><b>${escapeHTML(row.name)}</b><small>${row.wins} 胜 ${row.losses} 负</small></span>
      <span class="points${row.points < 0 ? ' negative' : ''}">${scoreText(row.points)}</span>
    </li>`).join('');
    setHTML(list, rows || '<li class="rank-empty">今天还没有棋手上榜，下一局就是你的。</li>');
    if (ownId) setHTML($(ownId), ownRankText(ranking));
  } catch {
    setHTML(list, '<li class="rank-empty">暂时无法读取排行榜。</li>');
  }
}

function watchSideRanking(active) {
  if (active && !sideRankingTimer) {
    refreshSideRanking();
    sideRankingTimer = setInterval(() => { if (!document.hidden) refreshSideRanking(); }, 30_000);
  } else if (!active && sideRankingTimer) {
    clearInterval(sideRankingTimer);
    sideRankingTimer = null;
  }
}

function watchHubRanking(active) {
  const refresh = () => {
    refreshSideRanking('#hub-rank-gomoku', null);
    ddz.refreshHubRanking();
  };
  if (active && !hubRankingTimer) {
    refresh();
    hubRankingTimer = setInterval(() => { if (!document.hidden) refresh(); }, 30_000);
  } else if (!active && hubRankingTimer) {
    clearInterval(hubRankingTimer);
    hubRankingTimer = null;
  }
}

function renderHub() {
  if (!state) return;
  const rooms = state.rooms;
  const tables = state.tables;
  const gomokuPlaying = rooms.filter(room => room.playing).length;
  const ddzPlaying = tables.filter(room => room.playing).length;
  setText($('#hub-stat-online'), String(state.online));
  setText($('#hub-stat-playing'), String(gomokuPlaying + ddzPlaying));
  const gomokuOpen = rooms.filter(room => room.joinable).length;
  const ddzWaiting = tables.filter(room => room.joinable && room.humans > 0).length;
  setText($('#hub-gomoku-chip span'), gomokuPlaying ? `${gomokuPlaying} 局对弈中` : `${gomokuOpen} 间可入座`);
  setText($('#hub-ddz-chip span'), ddzPlaying ? `${ddzPlaying} 桌对局中` : ddzWaiting ? `${ddzWaiting} 桌等人` : '机器人随时补位');
}

/* ---------- Board ---------- */

function buildBoard() {
  $('#coords-top').innerHTML = COLUMNS.map(column => `<span>${column}</span>`).join('');
  $('#coords-left').innerHTML = COLUMNS.map((_, index) => `<span>${index + 1}</span>`).join('');
  const fragment = document.createDocumentFragment();
  for (let index = 0; index < SIZE * SIZE; index++) {
    const point = document.createElement('button');
    point.type = 'button';
    point.className = 'point';
    point.dataset.index = index;
    point.dataset.coordinate = `${COLUMNS[index % SIZE]}${Math.floor(index / SIZE) + 1}`;
    point.tabIndex = index === boardFocus ? 0 : -1;
    point.innerHTML = '<span class="disc"></span>';
    points.push(point);
    fragment.append(point);
  }
  el.board.append(fragment);

  const hero = $('#hero-board');
  const opening = [[7, 7, 'b'], [8, 6, 'w'], [8, 8, 'b'], [6, 6, 'w'], [9, 9, 'b'], [10, 10, 'w'], [6, 8, 'b'], [7, 8, 'w'], [9, 7, 'b'],
    [6, 5, 'w'], [10, 8, 'b'], [5, 9, 'w'], [8, 9, 'b'], [7, 10, 'w'], [9, 8, 'b'], [11, 8, 'w'], [7, 6, 'b'], [5, 4, 'w'], [9, 6, 'b', 'last']];
  const position = value => `${(6.25 * (value + 1)).toFixed(3)}%`;
  hero.insertAdjacentHTML('beforeend', opening.map(([x, y, color, last]) => `<i class="mini-stone ${color}${last ? ' last' : ''}" style="left:${position(x)};top:${position(y)}"></i>`).join(''));
}

/* ---------- Room ---------- */

function matchMessage(room) {
  const { game, seats, seat } = room;
  if (seat === -1) {
    if (!connected) return ['clock', '正在重连', '连接恢复后会同步棋盘和对话。'];
    if (game?.status === 'finished') {
      const title = game.reason === 'draw' ? '和棋' : `${game.winner === 1 ? '黑棋' : '白棋'}获胜`;
      if (game.reason === 'timeout') return ['clock', title, `${game.turn === 1 ? '黑棋' : '白棋'}落子超时。`];
      return ['trophy', title, game.reason === 'left' ? '有一位棋手退出了本局。' : '本局已结束，可以留在棋室观看下一局。'];
    }
    if (game?.undo) return ['undo', `${game.undo.color === 1 ? '黑棋' : '白棋'}请求悔棋`, '等待对方回应，计时已暂停。'];
    if (game?.status === 'playing') return seats.every(person => person?.online)
      ? ['eye', '正在观战', game.moves === 0 ? '黑棋先行，棋盘会实时更新。' : '棋盘会随着棋手落子实时更新。']
      : ['clock', '棋手正在重连', '双方恢复连接后可以继续对弈。'];
    return ['eye', '等待开局', '两位棋手准备完成后会自动开局。'];
  }
  const self = seats[seat];
  const opponent = seats[1 - seat];
  if (!connected) return ['clock', '正在重连', '连接恢复后可以继续对弈。'];
  if (game?.status === 'finished') {
    if (game.reason === 'timeout') return game.winner === seat + 1
      ? ['trophy', '你获胜了', '对方落子超时。双方再次准备即可开局。']
      : ['clock', '你落子超时', '本局判对方获胜。双方再次准备即可开局。'];
    if (game.reason === 'draw') return ['trophy', '和棋', '棋盘已下满。双方再次准备即可开局。'];
    if (game.reason === 'left') return ['trophy', '你获胜了', '对方已离开。你可以邀请新的棋友入座。'];
    return ['trophy', game.winner === seat + 1 ? '你获胜了' : '对方获胜', '棋盘已标出连线。双方再次准备即可开局。'];
  }
  if (game?.undo) return game.undo.color === seat + 1
    ? ['undo', '已申请悔棋', '等待对方回应，计时已暂停；10 秒内未回应视为拒绝。']
    : ['undo', '对方请求悔棋', '请在弹窗中选择同意或拒绝，计时已暂停。'];
  if (!opponent) return ['users', '等待棋友入座', '复制邀请链接发给好友，或等待大厅中的棋友加入。'];
  if (!opponent.online) return ['clock', '对方正在重连', '棋室会为对方保留座位 45 秒。'];
  if (game?.status === 'playing') {
    if (game.turn === seat + 1) return ['grid', '轮到你落子', game.moves === 0 ? '你执黑，先落子。点击交叉点，或用方向键移动、回车落子。' : '点击空交叉点，或用方向键移动、回车落子；超时判负。'];
    return ['clock', '等待对方落子', game.moves === 0 ? '你执白，等待黑棋先行。' : '对方落子后，棋盘会实时更新。'];
  }
  if (self.ready) return ['check', '你已准备', '等待对方准备，双方就绪后自动开局。'];
  return ['check', '双方已入座', '点击「准备」，双方就绪后自动开局。'];
}

function renderSeat(node, room, index, isSelf) {
  const { game, seats } = room;
  const person = seats[index];
  const playing = game?.status === 'playing';
  const active = Boolean(person?.online && (playing ? game.turn === index + 1 : person.ready));
  const label = !person ? '空座' : !person.online ? '重连中' : playing ? (game.turn === index + 1 ? '落子中' : '等待') : person.ready ? '已准备' : '未准备';
  const color = playing || (game && !room.handicap) ? (index === 0 ? '黑棋' : '白棋')
    : room.handicap ? (room.handicap.giver === index ? `下局执白 · ${HANDICAP_LABELS[room.handicap.level]}` : '下局执黑 · 受让')
      : '开局时抽签分黑白';
  const name = person?.name ?? '等待棋友入座';

  node.classList.toggle('vacant', !person);
  node.classList.toggle('active', connected && active);
  const avatar = node.querySelector('.seat-avatar');
  avatar.className = `avatar avatar-lg seat-avatar ${person ? tone(person.name) : ''}`;
  setHTML(avatar, person ? avatarInner(person.name, person.avatar, game ? (index === 0 ? 'b' : 'w') : '') : icon('user'));
  setHTML(node.querySelector('.seat-name'), `${escapeHTML(name)}${isSelf ? ' <small>（你）</small>' : ''}`);
  node.querySelector('.seat-name').title = name;
  setText(node.querySelector('.seat-color'), color + (playing && game.turn === index + 1 && index === 0 && game.moves === 0 ? ' · 先行' : ''));
  const status = node.querySelector('.seat-state');
  setText(status, label);
  status.classList.toggle('on', connected && active);
  status.classList.toggle('off', Boolean(person && !person.online));
}

function renderWinRate(game, forecast) {
  const panel = $('#win-rate');
  const trend = $('#odds-history');
  const history = game?.blackWinHistory ?? [];
  const trendKey = JSON.stringify(history);
  if (trend.dataset.history !== trendKey) {
    trend.dataset.history = trendKey;
    const values = history.filter(value => value != null);
    trend.hidden = values.length < 2;
    if (!trend.hidden) {
      const parts = history.map((value, i) => value == null ? '' :
        `${i === 0 || history[i - 1] == null ? 'M' : 'L'}${300 * i / (history.length - 1)},${42 - 0.4 * value}h0.5`);
      trend.querySelector('path').setAttribute('d', parts.join(' '));
      setText(trend.querySelector('figcaption'), `黑棋胜率走势　${values[0]}% → ${values.at(-1)}%`);
    }
  }
  const details = $('#odds-details');
  details.hidden = trend.hidden && !(forecast?.variation?.length || forecast?.tactic || forecast?.explanation);
  const key = game ? JSON.stringify([game.status, game.winner, game.turn, game.board, forecast]) : null;
  if (key === oddsKey) return;
  oddsKey = key;
  oddsWorker?.terminate();
  oddsWorker = null;
  panel.hidden = !game;
  if (!game) return;

  const bar = $('#odds-bar');
  const values = $('#odds-values');
  function show([draw, black, white]) {
    setText(values, `黑胜 ${black}%　·　和棋 ${draw}%　·　白胜 ${white}%`);
    bar.style.setProperty('--black', black + '%');
    bar.style.setProperty('--white', white + '%');
    bar.style.visibility = 'visible';
  }

  const finished = game.status === 'finished';
  const titles = { rapfi: 'Rapfi 引擎胜率', tactic: '战术判定', model: '大模型预测胜率' };
  setText($('#odds-title'), finished ? '最终结果' : titles[forecast?.source] ?? '实时模拟胜率');
  const line = forecast?.variation?.map((cell, ply) => {
    const black = ply % 2 === 0 ? game.turn === 1 : game.turn === 2;
    return `${black ? '黑' : '白'} ${COLUMNS[cell % SIZE]}${Math.floor(cell / SIZE) + 1}`;
  }).join(' → ');
  for (const [selector, text] of [
    ['#odds-tactic', forecast?.tactic],
    ['#odds-line', line && `参考变化　${line}`],
    ['#odds-comment', forecast?.explanation && `大模型解读　${forecast.explanation}`]
  ]) {
    const node = $(selector);
    node.hidden = !text;
    setText(node, text ?? '');
  }
  if (finished) {
    show([0, 1, 2].map(color => color === (game.winner ?? 0) ? 100 : 0));
    return;
  }
  if (forecast) {
    show([forecast.draw, forecast.black, forecast.white]);
    return;
  }

  bar.style.visibility = 'hidden';
  setText(values, '正在模拟…');
  function unavailable() {
    if (key === oddsKey) setText(values, '本手估算暂不可用');
  }
  try {
    const worker = new Worker('/winrate.js');
    oddsWorker = worker;
    worker.onmessage = ({ data }) => {
      if (worker === oddsWorker) show(data);
      worker.terminate();
    };
    worker.onerror = unavailable;
    worker.postMessage({ board: game.board, turn: game.turn });
  } catch {
    unavailable();
  }
}

function renderRoom(room) {
  const { game, seats, seat } = room;
  const playing = game?.status === 'playing';
  const finished = game?.status === 'finished';
  const watching = seat === -1;
  const myTurn = !watching && connected && playing && game.turn === seat + 1 && seats.every(person => person?.online);

  setText($('#room-title'), room.name);
  setText($('#room-code'), `#${room.id}`);
  const handicapLevel = playing ? game.handicap : room.handicap?.level ?? 0;
  setText($('#room-facts'), handicapLevel ? `${RULES} · ${HANDICAP_LABELS[handicapLevel]} · 不计排行` : RULES);
  $('#watch-badge').hidden = !watching;
  $('#watch-invite').hidden = watching;
  setText($('#invite span'), watching ? '复制观战链接' : '复制邀请');
  setText($('#spectators span'), `${room.spectatorCount} 人观战`);
  setText($('#stat-spectators'), String(room.spectatorCount));
  setText($('#stat-moves'), !game ? '—' : playing ? `第 ${game.moves + 1} 手` : `${game.moves} 手`);
  setText($('#move-count'), `已落 ${game?.moves ?? 0} 子`);

  const chip = $('#board-state');
  chip.className = `chip ${!connected ? 'alert' : playing ? 'live' : 'wait'}`;
  setText(chip.querySelector('span'), !connected ? '正在重连'
    : playing ? (watching ? `${game.turn === 1 ? '黑棋' : '白棋'}回合` : myTurn ? `${game.turn === 1 ? '黑棋' : '白棋'}回合 · 轮到你落子` : `${game.turn === 1 ? '黑棋' : '白棋'}回合 · 对方思考中`)
    : finished ? '对局已结束' : '等待开局');

  el.board.setAttribute('aria-label', watching ? '观战棋盘，使用方向键查看落点' : '五子棋棋盘，使用方向键移动，按回车落子');
  el.board.dataset.preview = myTurn ? String(seat + 1) : '';
  const winning = new Set(game?.line);
  points.forEach((point, index) => {
    const stone = game?.board[index] ?? 0;
    point.classList.toggle('occupied', stone > 0);
    point.classList.toggle('last', game?.last === index);
    point.classList.toggle('win', winning.has(index));
    point.firstElementChild.classList.toggle('w', stone === 2);
    point.setAttribute('aria-label', `${point.dataset.coordinate}，${stone === 1 ? '黑棋' : stone === 2 ? '白棋' : '空位'}`);
    point.setAttribute('aria-disabled', String(!myTurn || stone > 0));
  });

  const lowerIndex = watching ? 0 : seat;
  el.lowerSeat.dataset.index = lowerIndex;
  el.upperSeat.dataset.index = 1 - lowerIndex;
  renderSeat(el.upperSeat, room, 1 - lowerIndex, false);
  renderSeat(el.lowerSeat, room, lowerIndex, !watching);

  const [symbol, title, description] = matchMessage(room);
  const status = $('#match-status');
  status.classList.toggle('go', myTurn);
  setHTML(status, `${icon(symbol)}<div><strong>${title}</strong><p>${description}</p></div>`);

  const ready = Boolean(seats[seat]?.ready);
  el.ready.hidden = playing || watching;
  el.ready.className = `btn btn-block ${ready ? 'btn-line' : 'btn-gold'}`;
  setText(el.ready.querySelector('span'), ready ? '取消准备' : finished && seats.every(Boolean) ? '再来一局' : '准备');
  el.takeSeat.hidden = !watching || playing || seats.every(Boolean);
  renderHandicap(room);
  renderUndo(room);

  renderChat(room);
  renderResult(room);
}

function renderHandicap(room) {
  const block = $('#handicap');
  block.hidden = room.seat === -1 || room.game?.status === 'playing';
  if (block.hidden) return;
  const handicap = room.handicap;
  const mine = handicap?.giver === room.seat;
  for (const button of block.querySelectorAll('[data-level]')) {
    const level = Number(button.dataset.level);
    button.setAttribute('aria-pressed', String(handicap ? mine && handicap.level === level : level === 0));
  }
  const note = $('#handicap-note');
  note.classList.toggle('mine', Boolean(handicap));
  setText(note, !handicap ? '让子局不计排行'
    : mine ? `你${HANDICAP_LABELS[handicap.level]}，将执白`
      : `对方${HANDICAP_LABELS[handicap.level]}给你，你将执黑`);
}

function renderUndo(room) {
  const { game, seat } = room;
  const playing = game?.status === 'playing';
  el.undo.hidden = seat === -1 || !playing;
  if (!el.undo.hidden) {
    const waiting = game.undo?.color === seat + 1;
    el.undo.disabled = !connected || room.undoColor !== seat + 1;
    setText(el.undo.querySelector('span'), waiting ? '等待对方回应…' : '悔棋');
    setText($('#undo-left'), `剩 ${game.undoLeft[seat]} 次`);
  }
  const asked = playing && game.undo && seat !== -1 && game.undo.color !== seat + 1;
  if (asked && !el.undoDialog.open) {
    const requester = room.seats[game.undo.color - 1]?.name ?? '对方';
    setText($('#undo-title'), `${requester} 请求悔棋`);
    for (const button of el.undoDialog.querySelectorAll('button')) button.disabled = !connected;
    el.undoDialog.showModal();
  } else if (!asked && el.undoDialog.open) el.undoDialog.close();
}

function renderClock() {
  const game = state?.room?.game;
  if (el.undoDialog.open && game?.undo) {
    const left = Math.max(0, Math.ceil((game.undo.deadline - (state.serverTime + performance.now() - syncedAt)) / 1000));
    setText($('#undo-timer'), `${left} 秒后自动拒绝`);
  }
  const show = connected && game?.status === 'playing';
  for (const seatNode of [el.upperSeat, el.lowerSeat]) {
    const clock = seatNode.querySelector('.seat-clock');
    const visible = show && Number(seatNode.dataset.index) === game.turn - 1;
    clock.hidden = !visible;
    if (!visible) continue;
    const paused = game.turnDeadline === null;
    const remaining = Math.max(0, paused ? game.turnRemaining : game.turnDeadline - (state.serverTime + performance.now() - syncedAt));
    const seconds = Math.ceil(remaining / 1000);
    setText(clock.querySelector('.clock-time'), `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`);
    setText(clock.querySelector('.clock-note'), !paused ? '剩余时间' : game.undo ? '悔棋请求中，已暂停' : '已暂停，等待重连');
    clock.querySelector('.clock-bar i').style.transform = `scaleX(${Math.min(1, remaining / TURN_MS).toFixed(4)})`;
    clock.classList.toggle('paused', paused);
    clock.classList.toggle('low', !paused && seconds <= 5);
  }
}

/* ---------- Chat & barrage ---------- */

function isSelfMessage(message, room) {
  return room.seat !== -1 ? message.seat === room.seat : message.seat === -1 && message.name === state.me.name;
}

function renderChat(room) {
  const { guidance } = room;
  const entering = el.chatLog.dataset.room !== room.id;
  if (entering) {
    el.chatLog.dataset.room = room.id;
    el.chatLog.replaceChildren();
    el.chatInput.value = '';
    clearBarrage();
  }
  const latest = Number(el.chatLog.lastElementChild?.dataset.id ?? 0);
  const follow = el.chatLog.scrollHeight - el.chatLog.scrollTop - el.chatLog.clientHeight < 32;
  el.chatLog.hidden = !guidance.length;
  $('#chat-empty').hidden = Boolean(guidance.length);
  const incoming = guidance.filter(message => message.id > latest);
  for (const message of incoming) {
    if (message.seat === -2) {
      el.chatLog.insertAdjacentHTML('beforeend', `<p class="msg-system" data-id="${message.id}">${escapeHTML(message.text)}</p>`);
      continue;
    }
    const role = message.seat === 0 ? '黑棋' : message.seat === 1 ? '白棋' : '观众';
    const self = isSelfMessage(message, room);
    el.chatLog.insertAdjacentHTML('beforeend', `<div class="msg${self ? ' self' : ''}" data-id="${message.id}">
      <span class="avatar ${tone(message.name)}" aria-hidden="true">${escapeHTML(initial(message.name))}</span>
      <div class="msg-body"><div class="msg-meta"><b>${escapeHTML(message.name)}</b><span class="role${message.seat === -1 ? ' viewer' : ''}">${role}</span></div><p>${escapeHTML(message.text)}</p></div>
    </div>`);
  }
  if (incoming.length && !entering && el.barrageToggle.getAttribute('aria-pressed') === 'true') {
    barrageQueue.push(...incoming.filter(message => message.seat !== -2).slice(-6).map(message => ({ ...message, self: isSelfMessage(message, room) })));
    if (barrageQueue.length > 6) barrageQueue.splice(0, barrageQueue.length - 6);
    showBarrage();
  }
  while (el.chatLog.children.length > guidance.length) el.chatLog.firstElementChild.remove();
  if (follow && guidance.at(-1)?.id > latest) el.chatLog.scrollTop = el.chatLog.scrollHeight;
  el.chatInput.disabled = !connected;
  setText($('#chat-jump span'), guidance.length ? `对话 ${guidance.length}` : '对话');
}

function showBarrage() {
  const busy = new Set([...el.barrage.children].map(item => item.dataset.lane));
  for (const lane of ['0', '1', '2']) {
    if (!barrageQueue.length) break;
    if (busy.has(lane)) continue;
    const message = barrageQueue.shift();
    const item = document.createElement('span');
    item.className = `barrage-item${message.self ? ' mine' : ''}`;
    item.dataset.lane = lane;
    item.style.setProperty('--lane', lane);
    item.innerHTML = `<b>${escapeHTML(message.name)}</b><span>${escapeHTML(message.text)}</span>`;
    item.addEventListener('animationend', () => {
      item.remove();
      showBarrage();
    }, { once: true });
    el.barrage.append(item);
  }
}

function clearBarrage() {
  barrageQueue.length = 0;
  el.barrage.replaceChildren();
}

/* ---------- Result ---------- */

function renderResult(room) {
  const game = room.game;
  const key = game ? `${room.id}:${game.status}:${game.moves}:${game.reason}:${game.winner}` : `${room.id}:none`;
  const previous = lastGameKey;
  lastGameKey = key;
  const justFinished = game?.status === 'finished' && previous !== key && previous?.startsWith(`${room.id}:playing:`);
  if (!justFinished) {
    if (game?.status === 'playing' && el.result.open) el.result.close();
    return;
  }

  const watching = room.seat === -1;
  const draw = game.reason === 'draw';
  const won = !watching && game.winner === room.seat + 1;
  const winnerColor = game.winner === 1 ? '黑棋' : '白棋';
  const seal = $('#result-seal');
  seal.className = `result-seal${draw ? ' draw' : !watching && !won ? ' lose' : ''}`;
  seal.textContent = draw ? '和' : watching || won ? '胜' : '负';

  const reasons = { line: '五子连珠', timeout: '超时判负', draw: '棋盘下满', left: '对手离场' };
  const reason = reasons[game.reason] ?? '对局结束';
  let title;
  let copy;
  if (draw) {
    title = '和棋';
    copy = '棋盘已下满，双方不分胜负。';
  } else if (watching) {
    title = `${winnerColor}获胜`;
    copy = game.reason === 'timeout' ? `${game.turn === 1 ? '黑棋' : '白棋'}落子超时。` : game.reason === 'left' ? '有一位棋手退出了本局。' : `${winnerColor}率先连成五子。`;
  } else if (won) {
    title = game.reason === 'left' ? '对方离场 · 你赢了' : `${winnerColor}${game.reason === 'timeout' ? '胜出' : '五连'} · 你赢了`;
    copy = game.reason === 'timeout' ? '对方落子超时，本局判你获胜。' : game.reason === 'left' ? '对方已离开棋室，本局判你获胜。' : '漂亮的一局，五子连珠已在棋盘上标出。';
  } else {
    title = game.reason === 'timeout' ? '落子超时 · 本局告负' : '对方获胜';
    copy = game.reason === 'timeout' ? '每手限时 30 秒，下局注意时间。' : '胜败乃兵家常事，再来一局扳回来。';
  }
  setText($('#result-title'), title);
  setText($('#result-copy'), game.handicap ? `${copy}（${HANDICAP_LABELS[game.handicap]}局，不计排行）` : copy);
  setText($('#result-moves'), String(game.moves));
  setText($('#result-reason'), reason);
  const pair = room.seats.every(Boolean);
  $('#result-again').hidden = watching;
  setText($('#result-again').querySelector('b'), pair ? '再来一局' : '准备下一局');
  setText($('#result-again-note'), pair ? '原班续局交换黑白' : '等待新棋友入座');
  if (!anyDialogOpen()) el.result.showModal();
}

/* ---------- Render ---------- */

function anyDialogOpen() {
  return [el.dialog, el.auth, el.ranking, el.result, el.undoDialog, ddz.resultDialog, $('#history-dialog')].some(node => node.open);
}

function switchView(next, push = true) {
  if (next === view) {
    window.scrollTo({ top: 0, behavior: 'smooth' });
    return;
  }
  view = next;
  if (push) {
    const room = view === 'gomoku' ? state?.room : view === 'ddz' ? state?.table : null;
    const url = new URL(PATHS[view], location.href);
    if (room) url.searchParams.set('room', room.id);
    if (room?.seat === -1) url.searchParams.set('view', 'watch');
    history.pushState(null, '', url);
  }
  render();
  window.scrollTo(0, 0);
}

function render() {
  const room = state?.room ?? null;
  const table = state?.table ?? null;
  // Sitting down anywhere (a link, a room number, quick play) brings that game forward.
  if (room?.id && room.id !== knownRoom) view = 'gomoku';
  if (table?.id && table.id !== knownTable) view = 'ddz';
  knownRoom = room?.id ?? null;
  knownTable = table?.id ?? null;
  renderHeader();

  const showRoom = view === 'gomoku' && room;
  const showTable = view === 'ddz' && table;
  el.hub.hidden = view !== 'hub';
  el.lobby.hidden = view !== 'gomoku' || Boolean(room);
  el.roomView.hidden = !showRoom;
  el.ddzLobby.hidden = view !== 'ddz' || Boolean(table);
  el.ddzView.hidden = !showTable;
  for (const [game, node] of [['hub', '#nav-hub'], ['gomoku', '#nav-lobby'], ['ddz', '#nav-ddz']]) {
    if (game === view) $(node).setAttribute('aria-current', 'page');
    else $(node).removeAttribute('aria-current');
  }
  const away = (room && view !== 'gomoku') ? room : (table && view !== 'ddz') ? table : null;
  $('#elsewhere').hidden = !away;
  if (away) setText($('#elsewhere-text'), `你还在${room ? '五子棋' : '斗地主'}「${away.name}」#${away.id}${away.seat === -1 ? '观战' : ''}`);

  const live = Boolean(state);
  watchSideRanking(live && view === 'gomoku' && !room);
  ddz.watchSide(live && view === 'ddz' && !table);
  watchHubRanking(live && view === 'hub');

  renderWinRate(showRoom ? room.game : null, room?.prediction);
  if (showRoom) renderRoom(room);
  else if (view === 'gomoku') renderLobby();
  if (showTable) ddz.renderTable(table);
  else if (view === 'ddz') ddz.renderLobby();
  if (view === 'hub') renderHub();
  renderClock();

  const shown = showRoom ? room : showTable ? table : null;
  const id = shown?.id ?? null;
  if (!requestedRoom && (id || visibleRoom)) syncURL(id, shown?.seat === -1);
  if (!room) {
    delete el.chatLog.dataset.room;
    lastGameKey = null;
    if (el.result.open) el.result.close();
  }
  if (table) atTable = true;
  else if (atTable) {
    atTable = false;
    ddz.leaveTable();
  }
  const key = `${view}:${id ?? ''}`;
  if (id !== visibleRoom || key !== visibleKey) {
    const entered = id && id !== visibleRoom;
    visibleRoom = id;
    visibleKey = key;
    if (!room) clearBarrage();
    window.scrollTo(0, 0);
    if (entered && !el.dialog.open) $(showTable ? '#ddz-title' : '#room-title').focus({ preventScroll: true });
  }
}

/* ---------- Dialogs ---------- */

const prompts = {
  create: { title: '创建房间', copy: '创建后你会直接入座，可以复制链接邀请好友。', label: '房间名称（选填）', placeholder: '例如：茶歇来一局', max: 20, submit: '创建并入座' },
  profile: { title: '编辑昵称', copy: '棋友会在对局和对话中看到这个昵称。', label: '棋手昵称', max: 16, submit: '保存昵称', required: true },
  leave: { title: '退出棋室？', copy: '对弈正在进行。现在退出，本局将判对方获胜。', submit: '确认退出' },
  rules: {
    title: '游戏规则',
    copy: '<p>弈间使用自由五子棋规则，棋盘 15 × 15。</p><ul><li>双方点击「准备」后自动开局。首局随机分配黑白，原班两人续局交换黑白，换棋手后重新抽签。</li><li>黑棋先行，双方轮流在空交叉点落一子。每手限时 30 秒，超时判负。</li><li>横、竖或斜线连成五颗或更多同色棋子即获胜；棋盘下满无人连五则为和棋。</li><li>让子：开局前由让方在棋室选择「让先 / 让二子 / 让三子」，被让方固定执黑。让二子、三子会先在 H8、I9（三子再加 G9）摆好黑子，由白棋先走。修改让子会取消双方准备，重新准备即视为同意；有人离座则恢复不让。让子局不计入排行。</li><li>悔棋：自己落子后、对方落子前，可申请撤回自己的上一手，需对方同意，10 秒未回应视为拒绝，等待期间暂停计时。同意后由你重新落子并重新计时。每人每局可申请 3 次，被拒绝的同一手不能再申请。</li><li>登录智算账号后对局计入每日排行：胜 +1、负 −1、和棋不变，北京时间 00:00 重置。</li><li>棋手断线会暂停计时并保留座位 45 秒，超时未回则判对方获胜。</li></ul>'
  },
  'ddz-create': { title: '创建牌桌', copy: '创建后你会直接入座，可以复制链接邀请牌友，人不够时请机器人补位。', label: '牌桌名称（选填）', placeholder: '例如：周五牌局', max: 20, submit: '创建并入座' },
  'ddz-leave': { title: '离开牌桌？', copy: '牌局正在进行。离开后由托管代打到本局结束，输赢照常结算。', submit: '确认离开' },
  'ddz-rules': {
    title: '斗地主规则',
    copy: '<p>一副 54 张牌，三人一桌，每人 17 张，留 3 张底牌。牌从小到大：3 4 5 6 7 8 9 10 J Q K A 2 小王 大王。</p><ul><li>叫地主：随机一人先叫。有人叫地主后，之后没说「不叫」的人可以抢地主，最先叫的人最后还能再抢一次。每抢一次倍数 ×2，三家都不叫就重新发牌。地主拿走底牌，底牌所有人可见。</li><li>加倍：定下地主后每人选择是否加倍（×2）。农民加倍只影响自己与地主之间的结算。地主在出第一手前可以明牌（×2）。</li><li>牌型：单张、对子、三张、三带一、三带一对、顺子（5 张起，3 到 A）、连对（3 对起）、飞机（两组以上相连三张，可带同数量的单张或对子）、四带二（两单或两对，不算炸弹）、炸弹（四张相同）、王炸（大小王）。</li><li>出牌：地主先出。跟牌须同牌型、同张数且更大；炸弹压一切非炸弹，大炸弹压小炸弹，王炸最大。每打出一个炸弹或王炸，倍数 ×2。</li><li>春天：地主出完而农民一张未出；反春：农民获胜而地主只出过一手。各 ×2。</li><li>结算：每位农民的输赢 = 底分 1 × 公共倍数 × 地主加倍 × 本人加倍，单人封顶 64 分，地主输赢为两位农民之和。</li><li>叫地主和加倍限时 15 秒，出牌限时 25 秒。超时或断线自动托管，可随时取消托管。</li><li>三位真人入座、至少一人登录智算账号时计入每日积分榜；有机器人的局和人机练习不计排行。</li></ul>'
  },
  'hub-rules': {
    title: '游戏规则',
    copy: '<p>弈间有两款游戏，进入对应大厅后点「游戏规则」可查看完整规则。</p><ul><li>五子棋：15 × 15 棋盘，黑棋先行，每手 30 秒，横竖斜任一方向先连成五子者胜。支持让子和悔棋。</li><li>斗地主：三人一桌，叫地主、抢地主、加倍，炸弹和王炸翻倍，先出完手牌的一方获胜。人不够时可以请机器人补位。</li><li>登录智算账号后，五子棋对局和三人真人斗地主计入每日排行，北京时间 00:00 重置。</li></ul>'
  },
  invite: { title: '邀请棋友', copy: '复制下面的链接发送给好友，打开即可入座。', label: '房间邀请链接' },
  'watch-invite': { title: '邀请观战', copy: '好友打开链接后会进入观战视角。', label: '房间观战链接' }
};

function openDialog(action, value = '') {
  const prompt = prompts[action];
  dialogAction = action;
  el.dialogTitle.textContent = prompt.title;
  el.dialogTitle.tabIndex = -1;
  $('#dialog-copy').innerHTML = prompt.copy;
  $('#account').hidden = action !== 'profile';
  $('#dialog-field').hidden = !prompt.label;
  $('#dialog-form').hidden = !prompt.label && !prompt.submit;
  $('#dialog-field label').textContent = prompt.label ?? '';
  el.dialogError.hidden = true;
  el.dialogInput.value = prompt.label ? value : '';
  el.dialogInput.placeholder = prompt.placeholder ?? '';
  el.dialogInput.required = Boolean(prompt.required);
  el.dialogInput.readOnly = !prompt.submit;
  el.dialogInput.maxLength = prompt.max ?? 4000;
  el.dialogSubmit.hidden = !prompt.submit;
  el.dialogSubmit.disabled = false;
  el.dialogSubmit.textContent = prompt.submit ?? '';
  for (const node of [el.auth, el.ranking, el.result, ddz.resultDialog]) if (node.open) node.close();
  el.dialog.showModal();
  if (prompt.label) {
    el.dialogInput.focus();
    if (value) el.dialogInput.select();
  } else el.dialogTitle.focus();
}

function setAuthStep(step) {
  authStep = step;
  const totp = step === 'totp';
  $('#auth-credentials').hidden = totp;
  $('#auth-totp').hidden = !totp;
  $('#auth-back').hidden = !totp;
  $('#auth-footer').hidden = totp;
  el.authEmail.disabled = totp;
  el.authPassword.disabled = totp;
  el.authCode.disabled = !totp;
  el.authCode.value = '';
  el.authError.hidden = true;
  $('#auth-title').textContent = totp ? '完成二次验证' : '使用智算账号登录';
  $('#auth-copy').textContent = totp ? '请输入身份验证器中的 6 位动态验证码' : '登录后对局计入每日排行';
  el.authSubmit.textContent = totp ? '验证并登录' : '登录';
  if (el.auth.open) (totp ? el.authCode : el.authEmail).focus();
}

function openAuth() {
  for (const node of [el.dialog, el.ranking, el.result]) if (node.open) node.close();
  setAuthStep('credentials');
  el.auth.showModal();
  el.authEmail.focus();
}

function setRankingGame(game) {
  rankingGame = game;
  for (const option of document.querySelectorAll('[data-action="rank-game"]')) option.setAttribute('aria-pressed', String(option.dataset.game === game));
  setText($('#rank-reset'), game === 'ddz' ? '北京时间 00:00 重置 · 底分 × 倍数 · 三人真人局计分' : '北京时间 00:00 重置 · 胜 +1 负 −1 和 0');
  $('#ranking-head').innerHTML = game === 'ddz'
    ? '<th scope="col">排名</th><th scope="col">玩家</th><th scope="col">局数</th><th scope="col">胜</th><th scope="col">积分</th>'
    : '<th scope="col">排名</th><th scope="col">棋手</th><th scope="col">胜</th><th scope="col">负</th><th scope="col">积分</th>';
}

function openRanking(game = view === 'ddz' ? 'ddz' : 'gomoku') {
  setRankingGame(game);
  for (const node of [el.dialog, el.auth, el.result, ddz.resultDialog]) if (node.open) node.close();
  el.rankingTable.hidden = true;
  el.rankingOwn.hidden = true;
  el.rankingRetry.hidden = true;
  el.podium.hidden = true;
  el.rankingStatus.hidden = false;
  el.rankingStatus.textContent = '正在读取排行榜…';
  el.ranking.showModal();
  $('#ranking-title').focus();
  refreshRanking();
  clearInterval(rankingTimer);
  rankingTimer = setInterval(refreshRanking, 12_000);
}

async function refreshRanking() {
  if (rankingPending) return;
  rankingPending = true;
  try {
    const game = rankingGame;
    const ranking = game === 'ddz'
      ? await fetch('/api/ddz/leaderboard', { signal: AbortSignal.timeout(7000) }).then(response => {
        if (!response.ok) throw new Error('Ranking unavailable');
        return response.json();
      })
      : await fetchRanking();
    if (!el.ranking.open || game !== rankingGame) return;
    const { entries } = ranking;
    const record = row => (game === 'ddz' ? `${row.games} 局 ${row.wins} 胜` : `${row.wins} 胜 ${row.losses} 负`);
    const slots = [entries[1], entries[0], entries[2]].map((row, index) => {
      const place = [2, 1, 3][index];
      if (!row) return `<div class="podium-slot p${place} empty-slot" aria-hidden="true"></div>`;
      return `<div class="podium-slot p${place}">
        <span class="avatar ${tone(row.name)}" aria-hidden="true">${escapeHTML(initial(row.name))}</span>
        <b title="${escapeHTML(row.name)}">${escapeHTML(row.name)}</b>
        <div class="podium-block"><strong>${row.rank}</strong><small>${scoreText(row.points)} 分 · ${record(row)}</small></div>
      </div>`;
    }).join('');
    setHTML(el.podium, slots);
    el.podium.hidden = !entries.length;
    $('#ranking-rows').innerHTML = entries.slice(3).map(row => `<tr><td>${row.rank}</td><th scope="row" title="${escapeHTML(row.name)}">${escapeHTML(row.name)}</th><td>${game === 'ddz' ? row.games : row.wins}</td><td>${game === 'ddz' ? row.wins : row.losses}</td><td class="points${row.points < 0 ? ' negative' : ''}">${scoreText(row.points)}</td></tr>`).join('');
    el.rankingTable.hidden = entries.length <= 3;
    el.rankingStatus.hidden = Boolean(entries.length);
    if (!entries.length) el.rankingStatus.textContent = game === 'ddz' ? '今天还没有三人真人局计分，组一桌就是你的。' : '今天还没有棋手上榜，下一局就是你的。';
    const account = state?.me?.account;
    el.rankingOwn.hidden = !account;
    el.rankingOwn.innerHTML = ranking.you
      ? `<span class="you">你</span><span>今日第 <b>${ranking.you.rank}</b> 名 · ${record(ranking.you)}</span><span class="points${ranking.you.points < 0 ? ' negative' : ''}">${scoreText(ranking.you.points)}</span>`
      : '<span class="you">你</span><span>今天还没有计分对局</span>';
    el.rankingRetry.hidden = true;
  } catch {
    if (!el.ranking.open) return;
    el.rankingTable.hidden = true;
    el.rankingOwn.hidden = true;
    el.podium.hidden = true;
    el.rankingStatus.hidden = false;
    el.rankingStatus.textContent = '暂时无法读取今日排行榜。';
    el.rankingRetry.hidden = false;
  } finally {
    rankingPending = false;
  }
}

async function copyInvite(watching) {
  const room = view === 'ddz' ? state?.table : state?.room;
  if (!room) return;
  const url = new URL(view === 'ddz' ? '/ddz' : '/', location.href);
  url.searchParams.set('room', room.id);
  if (watching) url.searchParams.set('view', 'watch');
  try {
    await navigator.clipboard.writeText(url.href);
    toast(watching ? '观战链接已复制' : '邀请链接已复制');
  } catch {
    openDialog(watching ? 'watch-invite' : 'invite', url.href);
  }
}

function showJoinError(message) {
  el.joinError.textContent = message;
  el.joinError.hidden = !message;
  el.joinCode.setAttribute('aria-invalid', String(Boolean(message)));
}

/* ---------- Events ---------- */

el.auth.addEventListener('cancel', event => { if (authBusy) event.preventDefault(); });
el.auth.addEventListener('close', () => el.authForm.reset());
el.ranking.addEventListener('close', () => clearInterval(rankingTimer));
// The request needs an answer; Esc must not silently dismiss it.
el.undoDialog.addEventListener('cancel', event => event.preventDefault());
el.rankingRetry.addEventListener('click', refreshRanking);
document.addEventListener('error', event => {
  if (event.target.matches?.('.avatar img')) event.target.remove();
}, true);

el.authForm.addEventListener('submit', async event => {
  event.preventDefault();
  if (authBusy) return;
  authBusy = true;
  el.authSubmit.disabled = true;
  el.authError.hidden = true;
  const totp = authStep === 'totp';
  const data = totp ? { code: el.authCode.value } : { email: el.authEmail.value, password: el.authPassword.value };
  el.authPassword.value = '';
  try {
    const result = await post(totp ? '/api/auth/totp' : '/api/auth/login', data, true);
    if (result.requiresTotp) setAuthStep('totp');
    else {
      el.auth.close();
      toast('已登录智算账号');
    }
  } catch (error) {
    el.authError.textContent = error.message;
    el.authError.hidden = false;
    const field = totp ? el.authCode : el.authPassword;
    field.focus();
    field.select();
  } finally {
    authBusy = false;
    el.authSubmit.disabled = false;
  }
});

$('#chat-form').addEventListener('submit', async event => {
  event.preventDefault();
  const draft = el.chatInput.value;
  const text = draft.trim();
  const roomId = state?.room?.id;
  if (!text || !roomId) return;
  if (await run('guidance', { text }) && state?.room?.id === roomId && el.chatInput.value === draft) el.chatInput.value = '';
});

el.joinCode.addEventListener('input', () => showJoinError(''));
$('#code-form').addEventListener('submit', async event => {
  event.preventDefault();
  const id = el.joinCode.value.trim();
  if (!/^\d{4}$/.test(id)) {
    showJoinError('请输入四位房间号。');
    el.joinCode.focus();
    return;
  }
  showJoinError('');
  if (await run('join', { id }, showJoinError)) el.joinCode.value = '';
});

function hubJoinError(message) {
  const node = $('#hub-join-error');
  node.textContent = message;
  node.hidden = !message;
  $('#hub-join-code').setAttribute('aria-invalid', String(Boolean(message)));
}
$('#hub-join-code').addEventListener('input', () => hubJoinError(''));
$('#hub-code-form').addEventListener('submit', async event => {
  event.preventDefault();
  const input = $('#hub-join-code');
  const id = input.value.trim();
  if (!/^\d{4}$/.test(id)) {
    hubJoinError('请输入四位房间号。');
    input.focus();
    return;
  }
  hubJoinError('');
  if (await run('join', { id }, hubJoinError)) input.value = '';
});

$('#dialog-form').addEventListener('submit', async event => {
  event.preventDefault();
  if (!dialogAction || !prompts[dialogAction].submit) return;
  const field = el.dialogInput.value.trim();
  const action = dialogAction;
  el.dialogError.hidden = true;
  el.dialogSubmit.disabled = true;
  const commands = {
    create: ['create', { name: field }],
    'ddz-create': ['ddz-create', { name: field }],
    profile: ['rename', { name: field }],
    leave: ['leave'],
    'ddz-leave': ['leave']
  };
  const [command, data] = commands[action];
  const ok = await run(command, data, message => {
    if (el.dialog.open && action === dialogAction) {
      el.dialogError.textContent = message;
      el.dialogError.hidden = false;
    }
  });
  if (!el.dialog.open || action !== dialogAction) return;
  el.dialogSubmit.disabled = false;
  if (!ok) return;
  el.dialog.close();
  if (action === 'profile') toast('昵称已更新');
  if (action === 'create' && !el.roomView.hidden) $('#room-title').focus({ preventScroll: true });
  if (action === 'ddz-create' && !el.ddzView.hidden) $('#ddz-title').focus({ preventScroll: true });
});

document.addEventListener('click', event => {
  const button = event.target.closest('button[data-action]');
  if (!button || button.disabled) return;
  const room = state?.room;
  switch (button.dataset.action) {
    case 'game':
      switchView(button.dataset.game);
      break;
    case 'back-to-room':
      switchView(state?.room ? 'gomoku' : 'ddz');
      break;
    case 'rank-game':
      setRankingGame(button.dataset.game);
      refreshRanking();
      break;
    case 'lobby':
      if (room?.game?.status === 'playing' && room.seat !== -1) openDialog('leave');
      else if (room) run('leave');
      else window.scrollTo({ top: 0, behavior: 'smooth' });
      break;
    case 'rules':
      openDialog(view === 'ddz' ? 'ddz-rules' : view === 'hub' ? 'hub-rules' : 'rules');
      break;
    case 'create':
      openDialog('create');
      break;
    case 'history':
      openHistory(view, state?.room?.id || state?.table?.id);
      break;
    case 'ranking':
      openRanking(button.dataset.game);
      break;
    case 'close-ranking':
      el.ranking.close();
      break;
    case 'profile':
      if (state?.me.account) openDialog('profile', state.me.name);
      else openAuth();
      break;
    case 'guest-profile':
      if (!authBusy) {
        el.auth.close();
        openDialog('profile', state?.me.name);
      }
      break;
    case 'sign-in':
      openAuth();
      break;
    case 'sign-out':
      button.disabled = true;
      post('/api/auth/logout', {}, true).then(() => {
        el.dialog.close();
        toast('已退出登录，可继续以访客身份对弈');
      }).catch(error => {
        el.dialogError.textContent = error.message;
        el.dialogError.hidden = false;
      }).finally(() => { button.disabled = false; });
      break;
    case 'close-auth':
      if (!authBusy) el.auth.close();
      break;
    case 'auth-back':
      if (!authBusy) setAuthStep('credentials');
      break;
    case 'close':
      el.dialog.close();
      break;
    case 'close-result':
      el.result.close();
      break;
    case 'result-again':
      el.result.close();
      if (room?.seats[room.seat] && !room.seats[room.seat].ready) run('ready', { ready: true });
      break;
    case 'quick':
      run('quick');
      break;
    case 'join':
    case 'watch':
      run(button.dataset.action, { id: button.dataset.id });
      break;
    case 'handicap':
      run('handicap', { level: Number(button.dataset.level) });
      break;
    case 'undo':
      run('undo');
      break;
    case 'undo-accept':
    case 'undo-reject':
      el.undoDialog.close();
      run('undo-reply', { accept: button.dataset.action === 'undo-accept' });
      break;
    case 'take-seat':
      if (room?.seat === -1) run('join', { id: room.id });
      break;
    case 'filter':
      filter = button.dataset.filter;
      for (const option of document.querySelectorAll('[data-filter]')) option.setAttribute('aria-pressed', String(option === button));
      renderLobby();
      break;
    case 'ready':
      if (room?.seats[room.seat]) run('ready', { ready: !room.seats[room.seat].ready });
      break;
    case 'invite':
    case 'watch-invite':
      copyInvite(button.dataset.action === 'watch-invite' || room?.seat === -1);
      break;
    case 'chat':
      el.chat.scrollIntoView({ behavior: 'smooth', block: 'start' });
      el.chatInput.focus({ preventScroll: true });
      break;
    case 'barrage': {
      const enabled = button.getAttribute('aria-pressed') !== 'true';
      button.setAttribute('aria-pressed', String(enabled));
      button.title = enabled ? '关闭棋盘弹幕' : '显示棋盘弹幕';
      if (!enabled) clearBarrage();
      try { localStorage.setItem(BARRAGE_KEY, String(enabled)); } catch { /* Keep the preference for this page only. */ }
      break;
    }
    default:
      ddz.handle(button.dataset.action, button);
  }
});

el.search.addEventListener('input', renderLobby);
el.board.addEventListener('click', event => {
  const point = event.target.closest('.point');
  if (point?.getAttribute('aria-disabled') === 'false') run('move', { index: Number(point.dataset.index) });
});
el.board.addEventListener('focusin', event => {
  if (!event.target.matches('.point')) return;
  points[boardFocus].tabIndex = -1;
  boardFocus = Number(event.target.dataset.index);
  points[boardFocus].tabIndex = 0;
});
el.board.addEventListener('keydown', event => {
  if (!event.target.matches('.point')) return;
  const offsets = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -SIZE, ArrowDown: SIZE };
  if (!(event.key in offsets)) return;
  event.preventDefault();
  if ((event.key === 'ArrowLeft' && boardFocus % SIZE === 0) || (event.key === 'ArrowRight' && boardFocus % SIZE === SIZE - 1)) return;
  points[boardFocus + offsets[event.key]]?.focus();
});

/* ---------- Start ---------- */

const ddz = createDdz({
  $, run, toast, escapeHTML, icon, tone, initial, avatarInner, scoreText, setText, setHTML,
  state: () => state,
  connected: () => connected,
  now: serverNow,
  openDialog,
  anyDialogOpen
});
window.addEventListener('popstate', () => switchView(viewFromPath(), false));

buildBoard();
try {
  if (localStorage.getItem(BARRAGE_KEY) === 'false') el.barrageToggle.setAttribute('aria-pressed', 'false');
} catch { /* Keep the default when storage is unavailable. */ }
setInterval(() => {
  renderClock();
  if (!el.ddzView.hidden) ddz.renderClock();
}, 150);
claimStoredIdentity(storage()).then(connect);
