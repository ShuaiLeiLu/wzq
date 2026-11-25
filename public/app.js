const icons = {
  user: '<circle cx="12" cy="8" r="3"/><path d="M5 20v-1a5 5 0 0 1 5-5h4a5 5 0 0 1 5 5v1"/>',
  edit: '<path d="m14 5 5 5m-4-6a2 2 0 0 1 3 0l2 2a2 2 0 0 1 0 3L9 20l-5 1 1-5Z"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  zap: '<path d="m13 2-9 12h7l-1 8 10-12h-7l1-8Z"/>',
  search: '<circle cx="10.5" cy="10.5" r="5.8"/><path d="m15 15 4.5 4.5"/>',
  'arrow-right': '<path d="M4 12h15m-6-6 6 6-6 6"/>',
  'arrow-left': '<path d="M20 12H5m6 6-6-6 6-6"/>',
  book: '<path d="M12 7a6 6 0 0 0-6-3H3v14h3a7 7 0 0 1 6 2 7 7 0 0 1 6-2h3V4h-3a6 6 0 0 0-6 3Zm0 0v13"/>',
  door: '<path d="M13 4h6v16h-6M3 12h11m-4-4 4 4-4 4"/>',
  check: '<path d="m5 12 4 4L19 6"/>',
  grid: '<rect x="4" y="4" width="16" height="16" rx="2"/><path d="M4 12h16M12 4v16"/>',
  board: '<rect x="4" y="4" width="16" height="16" rx="2"/><path d="M4 10h16M4 15h16M10 4v16m5-16v16"/><circle cx="10" cy="10" r="2.1" fill="currentColor" stroke="none"/><circle cx="15" cy="15" r="2.1" fill="currentColor" stroke="none"/>',
  users: '<circle cx="9" cy="8" r="3"/><path d="M3 20v-1a5 5 0 0 1 5-5h2a5 5 0 0 1 5 5v1m1-15a3 3 0 1 1 0 6m4 9v-1a5 5 0 0 0-3-4.6"/>',
  eye: '<path d="M2 12s3.6-6 10-6 10 6 10 6-3.6 6-10 6-10-6-10-6Z"/><circle cx="12" cy="12" r="2.5"/>',
  message: '<path d="M20 11.5a6.5 6.5 0 0 1-6.5 6.5H6l-3 2v-8.5A6.5 6.5 0 0 1 9.5 5h4a6.5 6.5 0 0 1 6.5 6.5Z"/><path d="M8 10h7m-7 4h4"/>',
  barrage: '<path d="M3 6h13M8 12h13M3 18h10"/>',
  link: '<path d="m10 13 4-4m-6 7-2 2a3.5 3.5 0 0 1-5-5l4-4a3.5 3.5 0 0 1 5 0m4 6a3.5 3.5 0 0 0 5 0l4-4a3.5 3.5 0 0 0-5-5l-2 2" transform="translate(1 0) scale(.92)"/>',
  clock: '<circle cx="12" cy="12" r="8"/><path d="M12 7v5l3 2"/>',
  award: '<circle cx="12" cy="9" r="5"/><path d="m8 13-2 8 6-3 6 3-2-8"/>',
  close: '<path d="m6 6 12 12M6 18 18 6"/>'
};

const $ = selector => document.querySelector(selector);
const lobby = $('#lobby');
const roomView = $('#room-view');
const roomList = $('#room-list');
const board = $('#board');
const dialog = $('#dialog');
const dialogTitle = $('#dialog-title');
const dialogInput = $('#dialog-input');
const dialogSubmit = $('#dialog-submit');
const dialogError = $('#dialog-error');
const authDialog = $('#auth-dialog');
const authForm = $('#auth-form');
const authEmail = $('#auth-email');
const authPassword = $('#auth-password');
const authCode = $('#auth-code');
const authError = $('#auth-error');
const authSubmit = $('#auth-submit');
const readyButton = $('#ready-button');
const guidanceLog = $('#guidance-log');
const guidanceInput = $('#guidance-input');
const turnClock = $('#turn-clock');
const turnSeconds = $('#turn-seconds');
const barrage = $('#barrage');
const barrageToggle = $('#barrage-toggle');
const search = $('#room-search');
const storageKey = 'yijian-session';
const barrageKey = 'yijian-barrage';
const barrageQueue = [];
const pending = new Set();

let state = null;
let token = null;
let events;
let connected = false;
let syncedAt = 0;
let retry;
let visibleRoom = null;
let renderedRooms = '';
let filter = 'all';
let boardFocus = 112;
let dialogAction = null;
let authBusy = false;
let authStep = 'credentials';
let toastTimeout;
const linkParams = new URL(location.href).searchParams;
let requestedRoom = linkParams.get('room');
const linkedAction = linkParams.get('view') === 'watch' ? 'watch' : 'join';

function icon(name) {
  return `<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">${icons[name]}</svg>`;
}

function fillIcons(root) {
  for (const item of root.querySelectorAll('[data-icon]')) item.innerHTML = icon(item.dataset.icon);
}

function escapeHTML(value) {
  const characters = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
  return String(value).replace(/[&<>"']/g, character => characters[character]);
}

async function post(path, data, authenticated = false) {
  const headers = { 'Content-Type': 'application/json' };
  if (authenticated) headers.Authorization = `Bearer ${token}`;
  let response;
  try {
    response = await fetch(path, { method: 'POST', headers, body: JSON.stringify(data), signal: AbortSignal.timeout(10_000) });
  } catch {
    throw new Error('连接暂时不可用，请稍后重试。');
  }
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || '操作未完成，请重试。');
  return result;
}

function storage(value) {
  try {
    if (value === undefined) return sessionStorage.getItem(storageKey);
    if (value === null) sessionStorage.removeItem(storageKey);
    else sessionStorage.setItem(storageKey, value);
  } catch {
    // The game can run with an in-memory identity when storage is blocked.
  }
  return null;
}

const sibling = typeof BroadcastChannel === 'function' ? new BroadcastChannel('yijian-tabs') : null;
sibling?.addEventListener('message', event => {
  if (event.data.check && event.data.check === token) sibling.postMessage({ occupied: token });
});

function claimStoredIdentity(value) {
  if (!value || !sibling) return Promise.resolve(value);
  return new Promise(resolve => {
    let occupied = false;
    const hearSibling = event => { if (event.data.occupied === value) occupied = true; };
    sibling.addEventListener('message', hearSibling);
    sibling.postMessage({ check: value });
    setTimeout(() => {
      sibling.removeEventListener('message', hearSibling);
      resolve(occupied ? null : value);
    }, 125);
  });
}

async function connect(identity) {
  clearTimeout(retry);
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
    $('#online span').textContent = '正在重连';
    retry = setTimeout(() => connect(identity), 1800);
  }
}

function syncURL(id, watching = false) {
  const url = new URL(location.href);
  if (id) url.searchParams.set('room', id);
  else url.searchParams.delete('room');
  if (watching) url.searchParams.set('view', 'watch');
  else url.searchParams.delete('view');
  if (url.href !== location.href) history.replaceState(null, '', url);
}

function renderLobby() {
  const online = $('#online');
  online.classList.toggle('connected', connected);
  online.querySelector('span').textContent = connected ? `${state.online} 位在线` : state ? '正在重连' : '正在连接';
  if (!state) return;
  const query = search.value.trim().toLowerCase().replace(/^#/, '');
  const matches = state.rooms.filter(room => (filter === 'all' || room.joinable)
    && (room.name.toLowerCase().includes(query) || room.id.includes(query)));
  const signature = JSON.stringify([connected, matches, state.rooms.length]);
  if (signature === renderedRooms) return;
  renderedRooms = signature;
  const focused = roomList.contains(document.activeElement) ? { ...document.activeElement.dataset } : null;
  roomList.setAttribute('aria-busy', 'false');
  roomList.innerHTML = matches.length ? matches.map(room => {
    const status = room.playing ? '对弈中' : !room.joinable ? '已满员' : room.count ? '等一位' : '等待入座';
    const tone = room.playing ? 'playing' : room.joinable ? '' : 'full';
    const watch = room.watchable ? `<button class="button button-outline watch-button" type="button" data-action="watch" data-id="${room.id}" ${connected ? '' : 'disabled'}><span data-icon="eye">${icon('eye')}</span>观战</button>` : '';
    const join = room.joinable || !room.watchable ? `<button class="button button-outline join-button" type="button" data-action="join" data-id="${room.id}" ${room.joinable && connected ? '' : 'disabled'}>${room.joinable ? '进入房间' : '暂不可加入'}<span data-icon="arrow-right">${icon('arrow-right')}</span></button>` : '';
    return `<article class="room-row">
      <div class="room-id"><span class="room-emblem ${room.theme}" aria-hidden="true">${icon('board')}</span>
        <div class="room-ident"><h3 title="${escapeHTML(room.name)}">${escapeHTML(room.name)}</h3><span class="room-number">#${room.id}<span class="mobile-count"> · ${room.count}/2</span></span></div>
      </div>
      <span class="occupancy" aria-label="${room.count} 人，最多 2 人"><span data-icon="users">${icon('users')}</span>${room.count} / 2</span>
      <span class="room-status ${tone}"><i></i>${status}</span>
      <div class="room-actions">${watch}${join}</div>
    </article>`;
  }).join('') : '<p class="empty-state">没有符合条件的房间，试试其他筛选条件。</p>';
  if (focused?.id) {
    const fallback = roomList.querySelector(`[data-id="${focused.id}"]:not(:disabled)`);
    const sameAction = roomList.querySelector(`[data-id="${focused.id}"][data-action="${focused.action}"]:not(:disabled)`);
    (sameAction ?? fallback)?.focus();
  }
  $('#room-count').textContent = matches.length === state.rooms.length ? `共 ${matches.length} 个房间` : `显示 ${matches.length} / ${state.rooms.length} 个房间`;
}

const coordinates = Array.from({ length: 15 }, (_, index) => String.fromCharCode(65 + index));
const points = [];

function buildBoard() {
  for (const [index, column] of coordinates.entries()) {
    $('#columns').insertAdjacentHTML('beforeend', `<span>${column}</span>`);
    $('#rows').insertAdjacentHTML('beforeend', `<span>${index + 1}</span>`);
  }
  const fragment = document.createDocumentFragment();
  for (let index = 0; index < 225; index++) {
    const row = Math.floor(index / 15);
    const col = index % 15;
    const point = document.createElement('button');
    point.type = 'button';
    point.className = 'point';
    if ((row === 7 && col === 7) || ([3, 11].includes(row) && [3, 11].includes(col))) point.classList.add('star');
    point.dataset.index = index;
    point.dataset.coordinate = `${coordinates[col]}${row + 1}`;
    point.tabIndex = index === boardFocus ? 0 : -1;
    point.innerHTML = '<span class="stone-disc" aria-hidden="true"></span>';
    points.push(point);
    fragment.append(point);
  }
  board.append(fragment);
}

function matchMessage(room) {
  const { game, seats, seat } = room;
  if (seat === -1) {
    if (!connected) return ['clock', '正在重连', '连接恢复后会同步棋盘和对话。'];
    if (game?.status === 'finished') {
      const title = game.reason === 'draw' ? '和棋' : `${game.winner === 1 ? '黑棋' : '白棋'}获胜`;
      if (game.reason === 'timeout') return ['clock', title, `${game.turn === 1 ? '黑棋' : '白棋'}落子超时。`];
      const description = game.reason === 'left' ? '有一位棋手退出了本局。' : '本局已结束，可以留在棋室观看下一局。';
      return ['award', title, description];
    }
    if (game?.status === 'playing') return seats.every(person => person?.online)
      ? ['eye', '正在观战', '棋盘会随着棋手落子实时更新。']
      : ['clock', '棋手正在重连', '双方恢复连接后可以继续对弈。'];
    return ['eye', '等待开局', '两位棋手准备完成后会自动开局。'];
  }
  const self = seats[seat];
  const opponent = seats[1 - seat];
  if (!connected) return ['clock', '正在重连', '连接恢复后可以继续对弈。'];
  if (game?.status === 'finished') {
    if (game.reason === 'timeout') return game.winner === seat + 1
      ? ['award', '你获胜了', '对方落子超时。双方再次准备即可开局。']
      : ['clock', '你落子超时', '本局判对方获胜。双方再次准备即可开局。'];
    if (game.reason === 'draw') return ['award', '和棋', '棋盘已下满。双方再次准备即可开局。'];
    if (game.reason === 'left') return ['award', '你获胜了', '对方已离开。你可以邀请新的棋友入座。'];
    return ['award', game.winner === seat + 1 ? '你获胜了' : '对方获胜', '棋盘已标出连线。双方再次准备即可开局。'];
  }
  if (!opponent) return ['users', '等待棋友入座', '把邀请链接发给好友，或等待大厅中的棋友加入。'];
  if (!opponent.online) return ['clock', '对方正在重连', '棋室会为对方保留座位 45 秒。'];
  if (game?.status === 'playing') {
    return game.turn === seat + 1 ? ['grid', '轮到你落子', '选择棋盘上的一个空交叉点。'] : ['clock', '等待对方落子', '对方落子后，棋盘会实时更新。'];
  }
  if (self.ready) return ['check', '你已准备', '等待对方准备，双方就绪后自动开局。'];
  return ['check', '双方已入座', '点击准备。双方就绪后自动开局。'];
}

function renderClock() {
  const game = state?.room?.game;
  turnClock.hidden = !connected || game?.status !== 'playing';
  if (turnClock.hidden) return;
  const paused = game.turnDeadline === null;
  const serverNow = state.serverTime + performance.now() - syncedAt;
  const remaining = paused ? game.turnRemaining : game.turnDeadline - serverNow;
  const seconds = Math.max(0, Math.ceil(remaining / 1000));
  const label = paused ? `暂停 ${seconds}s` : `${seconds}s`;
  if (turnSeconds.textContent !== label) turnSeconds.textContent = label;
  turnClock.classList.toggle('paused', paused);
  turnClock.classList.toggle('low', !paused && seconds <= 5);
}

function showBarrage() {
  const lanes = new Set([...barrage.children].map(item => item.dataset.lane));
  for (const lane of ['0', '1', '2']) {
    if (!barrageQueue.length) break;
    if (lanes.has(lane)) continue;
    const message = barrageQueue.shift();
    const entry = document.createElement('span');
    entry.className = 'barrage-message';
    entry.dataset.lane = lane;
    entry.style.setProperty('--lane', lane);
    entry.innerHTML = `<span class="barrage-author">${escapeHTML(message.name)}</span><span class="barrage-text">${escapeHTML(message.text)}</span>`;
    entry.addEventListener('animationend', () => {
      entry.remove();
      showBarrage();
    }, { once: true });
    barrage.append(entry);
  }
}

function clearBarrage() {
  barrageQueue.length = 0;
  barrage.replaceChildren();
}

function renderGuidance(room) {
  const { guidance } = room;
  const entering = guidanceLog.dataset.room !== room.id;
  if (entering) {
    guidanceLog.dataset.room = room.id;
    guidanceLog.replaceChildren();
    guidanceInput.value = '';
    clearBarrage();
  }
  const latest = Number(guidanceLog.lastElementChild?.dataset.id ?? 0);
  const follow = guidanceLog.scrollHeight - guidanceLog.scrollTop - guidanceLog.clientHeight < 32;
  guidanceLog.hidden = !guidance.length;
  $('#guidance-empty').hidden = Boolean(guidance.length);
  const incoming = guidance.filter(message => message.id > latest);
  for (const message of incoming) {
    const role = message.seat === 0 ? '黑棋' : message.seat === 1 ? '白棋' : '观众';
    guidanceLog.insertAdjacentHTML('beforeend', `<div class="guidance-entry" data-id="${message.id}"><div class="message-author"><span class="message-role ${message.seat === -1 ? 'spectator' : ''}">${role}</span><span>${escapeHTML(message.name)}</span></div><p>${escapeHTML(message.text)}</p></div>`);
  }
  if (incoming.length && !entering && barrageToggle.getAttribute('aria-pressed') === 'true') {
    barrageQueue.push(...incoming.slice(-6));
    if (barrageQueue.length > 6) barrageQueue.splice(0, barrageQueue.length - 6);
    showBarrage();
  }
  while (guidanceLog.children.length > guidance.length) guidanceLog.firstElementChild.remove();
  if (follow && guidance.at(-1)?.id > latest) guidanceLog.scrollTop = guidanceLog.scrollHeight;
  guidanceInput.disabled = !connected;
  $('#guidance-jump').lastElementChild.textContent = guidance.length ? `对话 ${guidance.length}` : '对话';
}

function renderRoom(room) {
  const { game, seats, seat } = room;
  const playing = game?.status === 'playing';
  const finished = game?.status === 'finished';
  const watching = seat === -1;
  const color = seat + 1;
  const myTurn = !watching && connected && playing && game.turn === color && seats.every(person => person?.online);
  $('#room-title').textContent = room.name;
  $('#room-code').textContent = `房间 #${room.id}`;
  $('#watch-label').hidden = !watching;
  $('#room-invite').lastElementChild.textContent = watching ? '复制观战链接' : '复制邀请';
  $('#watch-invite').hidden = watching;
  $('#leave-room').lastElementChild.textContent = watching ? '退出观战' : '退出房间';
  $('#spectator-count').lastElementChild.textContent = `${room.spectatorCount} 人观战`;
  board.setAttribute('aria-label', watching ? '观战棋盘，使用方向键查看落点' : '五子棋棋盘，使用方向键移动，按回车落子');
  const chip = $('#board-state');
  chip.classList.toggle('active', connected && playing);
  chip.querySelector('span').textContent = !connected ? '正在重连' : playing ? `${game.turn === 1 ? '黑棋' : '白棋'}回合` : finished ? '对局已结束' : '等待开局';
  $('#move-count').textContent = `已落 ${game?.moves ?? 0} 子`;
  board.dataset.preview = color;
  const winning = new Set(game?.line);
  points.forEach((point, index) => {
    const stone = game?.board[index] ?? 0;
    point.classList.toggle('occupied', stone > 0);
    point.classList.toggle('last', game?.last === index);
    point.classList.toggle('win', winning.has(index));
    point.firstElementChild.classList.toggle('white', stone === 2);
    point.setAttribute('aria-label', `${point.dataset.coordinate}，${stone === 1 ? '黑棋' : stone === 2 ? '白棋' : '空位'}`);
    point.setAttribute('aria-disabled', String(!myTurn || stone > 0));
  });
  const seatsContent = seats.map((person, index) => {
    const mine = index === seat;
    const active = playing ? game.turn === index + 1 : person?.ready;
    const label = !person ? '空座' : !person.online ? '重连中' : playing ? active ? '落子中' : '等待落子' : person.ready ? '已准备' : '未准备';
    return `<div class="seat ${mine ? 'mine' : ''} ${person ? '' : 'vacant'}">
      <div class="seat-piece" aria-hidden="true"><i class="stone-disc ${index ? 'white' : ''}"></i></div>
      <div><div class="seat-name">${person?.avatar ? `<img src="${escapeHTML(person.avatar)}" alt="" referrerpolicy="no-referrer">` : ''}${person ? escapeHTML(person.name) : '等待加入'}</div><div class="seat-color">${index ? '白棋' : '黑棋'}${mine ? ' · 你' : ''}</div></div>
      <span class="seat-state ${active && person?.online ? 'highlight' : ''}">${label}</span>
    </div>`;
  }).join('');
  const seatsElement = $('#seats');
  if (seatsElement.dataset.rendered !== seatsContent) {
    seatsElement.innerHTML = seatsContent;
    seatsElement.dataset.rendered = seatsContent;
  }
  const [symbol, title, description] = matchMessage(room);
  const status = $('#match-status');
  const summary = `${title} ${description}`;
  if (status.dataset.message !== summary) {
    status.innerHTML = `<span>${icon(symbol)}</span><div><strong>${title}</strong><p>${description}</p></div>`;
    status.dataset.message = summary;
  }
  const ready = Boolean(seats[seat]?.ready);
  readyButton.hidden = playing || watching;
  readyButton.lastElementChild.textContent = ready ? '取消准备' : finished && seats.every(Boolean) ? '再来一局' : '准备';
  readyButton.classList.toggle('button-outline', ready);
  readyButton.classList.toggle('button-primary', !ready);
  $('#take-seat').hidden = !watching || playing || seats.every(Boolean);
  $('.game-side').classList.toggle('playing', playing || finished || watching);
  renderGuidance(room);
}

function render() {
  for (const button of document.querySelectorAll('[data-network]')) button.disabled = !connected;
  if (state) {
    $('#profile-name').textContent = state.me.name;
    const account = state.me.account;
    const profile = $('[data-action="profile"]');
    profile.setAttribute('aria-label', account ? '账号与昵称' : '登录');
    $('#profile-login-label').hidden = Boolean(account);
    $('#account-name').hidden = !account;
    $('#account-name').textContent = account ? `智算账号 · ${account.username}` : '';
    $('#account-login').hidden = Boolean(account);
    $('#account-logout').hidden = !account;
    const avatar = $('#profile-avatar');
    const url = account?.avatar || '';
    if (url !== (avatar.getAttribute('src') ?? '')) {
      avatar.hidden = !url;
      if (url) avatar.src = url;
      else avatar.removeAttribute('src');
    }
  }
  const room = state?.room;
  lobby.hidden = Boolean(room);
  roomView.hidden = !room;
  const navLobby = $('.main-nav [data-action="lobby"]');
  navLobby.classList.toggle('current', !room);
  if (room) navLobby.removeAttribute('aria-current');
  else navLobby.setAttribute('aria-current', 'page');
  if (room) renderRoom(room);
  else renderLobby();
  renderClock();
  const id = room?.id ?? null;
  if (id || visibleRoom) syncURL(id, room?.seat === -1);
  if (id !== visibleRoom) {
    visibleRoom = id;
    if (!id) {
      delete guidanceLog.dataset.room;
      clearBarrage();
    }
    window.scrollTo(0, 0);
    if (id && !dialog.open) $('#room-title').focus({ preventScroll: true });
    else if (!id) navLobby.focus({ preventScroll: true });
  }
}

function toast(message) {
  const element = $('#toast');
  element.textContent = message;
  element.hidden = false;
  clearTimeout(toastTimeout);
  toastTimeout = setTimeout(() => { element.hidden = true; }, 4200);
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

const prompts = {
  create: { title: '创建房间', copy: '创建后你会直接入座，可以复制链接邀请好友。', label: '房间名称（选填）', placeholder: '例如：茶歇来一局', max: 20, submit: '创建并入座' },
  profile: { title: '编辑昵称', copy: '棋友会在对局中看到这个昵称。', label: '棋手昵称', max: 16, submit: '保存昵称', required: true },
  'enter-code': { title: '用房间号加入', copy: '输入好友发来的四位房间号。', label: '房间号', placeholder: '例如：1001', max: 4, submit: '进入房间', required: true },
  leave: { title: '退出房间？', copy: '对弈正在进行。现在退出，本局将判对方获胜。', submit: '确认退出' },
  rules: { title: '游戏规则', copy: '<p>弈间使用自由五子棋规则。</p><ul><li>两人入座后分别准备，双方就绪时自动开局。</li><li>黑棋先行，双方轮流在空交叉点落一子。每手限时 30 秒，落子超时判负。</li><li>横、竖或斜线连成五颗或更多同色棋子，即可获胜。</li><li>棋盘下满且无人连成五子，则为和棋。</li></ul>' },
  invite: { title: '邀请棋友', copy: '复制下面的链接发送给好友。', label: '房间邀请链接' },
  'watch-invite': { title: '邀请观战', copy: '好友打开链接后会进入观战视角。', label: '房间观战链接' }
};

function openDialog(action, value = '') {
  const prompt = prompts[action];
  dialogAction = action;
  dialogTitle.textContent = prompt.title;
  dialogTitle.tabIndex = -1;
  $('#dialog-content').innerHTML = prompt.copy;
  $('#account-summary').hidden = action !== 'profile';
  $('#dialog-field').hidden = !prompt.label;
  $('#dialog-form').hidden = !prompt.label && !prompt.submit;
  $('#dialog-field label').textContent = prompt.label ?? '';
  dialogError.hidden = true;
  dialogInput.value = prompt.label ? value : '';
  dialogInput.placeholder = prompt.placeholder ?? '';
  dialogInput.required = Boolean(prompt.required);
  dialogInput.readOnly = !prompt.submit;
  dialogInput.inputMode = action === 'enter-code' ? 'numeric' : 'text';
  dialogInput.maxLength = prompt.max ?? 4000;
  dialogInput.pattern = action === 'enter-code' ? '[0-9]{4}' : '.*';
  dialogSubmit.hidden = !prompt.submit;
  dialogSubmit.disabled = false;
  dialogSubmit.textContent = prompt.submit ?? '';
  dialog.showModal();
  if (prompt.label) {
    dialogInput.focus();
    if (value) dialogInput.select();
  } else dialogTitle.focus();
}

function setAuthStep(step) {
  authStep = step;
  const totp = step === 'totp';
  $('#auth-credentials').hidden = totp;
  $('#auth-totp').hidden = !totp;
  $('#auth-back').hidden = !totp;
  $('#auth-footer').hidden = totp;
  authEmail.disabled = totp;
  authPassword.disabled = totp;
  authCode.disabled = !totp;
  authCode.value = '';
  authError.hidden = true;
  $('#auth-title').textContent = totp ? '完成二次验证' : '使用智算账号登录';
  $('#auth-copy').textContent = totp ? '请输入身份验证器中智算账号的 6 位动态验证码。' : '输入你的智算账号邮箱和密码。';
  authSubmit.textContent = totp ? '验证并登录' : '登录';
  if (authDialog.open) (totp ? authCode : authEmail).focus();
}

function openAuth() {
  if (dialog.open) dialog.close();
  setAuthStep('credentials');
  authDialog.showModal();
  authEmail.focus();
}

authDialog.addEventListener('cancel', event => { if (authBusy) event.preventDefault(); });
authDialog.addEventListener('close', () => authForm.reset());
document.addEventListener('error', event => {
  if (event.target.matches?.('#profile-avatar, .seat-name img')) event.target.hidden = true;
}, true);

authForm.addEventListener('submit', async event => {
  event.preventDefault();
  if (authBusy) return;
  authBusy = true;
  authSubmit.disabled = true;
  authError.hidden = true;
  const totp = authStep === 'totp';
  const data = totp ? { code: authCode.value } : { email: authEmail.value, password: authPassword.value };
  authPassword.value = '';
  try {
    const result = await post(totp ? '/api/auth/totp' : '/api/auth/login', data, true);
    if (result.requiresTotp) setAuthStep('totp');
    else {
      authDialog.close();
      toast('已登录智算账号');
    }
  } catch (error) {
    authError.textContent = error.message;
    authError.hidden = false;
    const field = totp ? authCode : authPassword;
    field.focus();
    field.select();
  } finally {
    authBusy = false;
    authSubmit.disabled = false;
  }
});

async function copyInvite(watching) {
  if (!state?.room) return;
  const url = new URL('/', location.href);
  url.searchParams.set('room', state.room.id);
  if (watching) url.searchParams.set('view', 'watch');
  try {
    await navigator.clipboard.writeText(url.href);
    toast(watching ? '观战链接已复制' : '邀请链接已复制');
  } catch {
    openDialog(watching ? 'watch-invite' : 'invite', url.href);
  }
}

$('#guidance-form').addEventListener('submit', async event => {
  event.preventDefault();
  const draft = guidanceInput.value;
  const text = draft.trim();
  const roomId = state?.room?.id;
  if (!text || !roomId) return;
  if (await run('guidance', { text }) && state?.room?.id === roomId && guidanceInput.value === draft) guidanceInput.value = '';
});

$('#dialog-form').addEventListener('submit', async event => {
  event.preventDefault();
  if (!dialogAction || !prompts[dialogAction].submit) return;
  const field = dialogInput.value.trim();
  const action = dialogAction;
  dialogError.hidden = true;
  dialogSubmit.disabled = true;
  const commands = {
    create: ['create', { name: field }],
    profile: ['rename', { name: field }],
    'enter-code': ['join', { id: field }],
    leave: ['leave']
  };
  const [command, data] = commands[action];
  const ok = await run(command, data, message => {
    if (dialog.open && action === dialogAction) {
      dialogError.textContent = message;
      dialogError.hidden = false;
    }
  });
  if (!dialog.open || action !== dialogAction) return;
  dialogSubmit.disabled = false;
  if (!ok) return;
  dialog.close();
  if (action === 'profile') toast('昵称已更新');
  if ((action === 'create' || action === 'enter-code') && !roomView.hidden) $('#room-title').focus({ preventScroll: true });
});

document.addEventListener('click', event => {
  const button = event.target.closest('button[data-action]');
  if (!button || button.disabled) return;
  switch (button.dataset.action) {
    case 'lobby':
      if (state?.room?.game?.status === 'playing' && state.room.seat !== -1) openDialog('leave');
      else if (state?.room) run('leave');
      else window.scrollTo(0, 0);
      break;
    case 'rules':
    case 'create':
    case 'enter-code':
      openDialog(button.dataset.action);
      break;
    case 'profile':
      if (state?.me.account) openDialog('profile', state.me.name);
      else openAuth();
      break;
    case 'guest-profile':
      if (!authBusy) {
        authDialog.close();
        openDialog('profile', state?.me.name);
      }
      break;
    case 'sign-in':
      openAuth();
      break;
    case 'sign-out':
      button.disabled = true;
      post('/api/auth/logout', {}, true).then(() => {
        dialog.close();
        toast('已退出登录，可继续以访客身份对弈');
      }).catch(error => {
        dialogError.textContent = error.message;
        dialogError.hidden = false;
      }).finally(() => { button.disabled = false; });
      break;
    case 'close-auth':
      if (!authBusy) authDialog.close();
      break;
    case 'auth-back':
      if (!authBusy) setAuthStep('credentials');
      break;
    case 'close':
      dialog.close();
      break;
    case 'quick':
      run('quick');
      break;
    case 'join':
    case 'watch':
      run(button.dataset.action, { id: button.dataset.id });
      break;
    case 'take-seat': {
      const room = state?.room;
      if (room?.seat === -1) run('join', { id: room.id });
      break;
    }
    case 'filter':
      filter = button.dataset.filter;
      for (const option of document.querySelectorAll('[data-filter]')) option.setAttribute('aria-pressed', String(option === button));
      renderLobby();
      break;
    case 'ready': {
      const room = state?.room;
      if (room?.seats[room.seat]) run('ready', { ready: !room.seats[room.seat].ready });
      break;
    }
    case 'invite':
    case 'watch-invite':
      copyInvite(button.dataset.action === 'watch-invite' || state?.room?.seat === -1);
      break;
    case 'guidance-panel':
    case 'board-panel': {
      const target = button.dataset.action === 'guidance-panel' ? $('#guidance-title') : $('#board-toolbar');
      target.focus({ preventScroll: true });
      target.scrollIntoView({ block: 'start' });
      break;
    }
    case 'barrage': {
      const enabled = button.getAttribute('aria-pressed') !== 'true';
      button.setAttribute('aria-pressed', String(enabled));
      if (!enabled) clearBarrage();
      try { localStorage.setItem(barrageKey, String(enabled)); } catch { /* Keep the preference for this page. */ }
      break;
    }
  }
});

search.addEventListener('input', renderLobby);
board.addEventListener('click', event => {
  const point = event.target.closest('.point');
  if (point?.getAttribute('aria-disabled') === 'false') run('move', { index: Number(point.dataset.index) });
});
board.addEventListener('focusin', event => {
  if (!event.target.matches('.point')) return;
  points[boardFocus].tabIndex = -1;
  boardFocus = Number(event.target.dataset.index);
  points[boardFocus].tabIndex = 0;
});
board.addEventListener('keydown', event => {
  if (!event.target.matches('.point')) return;
  const offsets = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -15, ArrowDown: 15 };
  if (!(event.key in offsets)) return;
  event.preventDefault();
  if ((event.key === 'ArrowLeft' && boardFocus % 15 === 0) || (event.key === 'ArrowRight' && boardFocus % 15 === 14)) return;
  points[boardFocus + offsets[event.key]]?.focus();
});

fillIcons(document);
buildBoard();
try {
  if (localStorage.getItem(barrageKey) === 'false') barrageToggle.setAttribute('aria-pressed', 'false');
} catch { /* Keep the default when storage is unavailable. */ }
setInterval(renderClock, 150);
claimStoredIdentity(storage()).then(connect);
