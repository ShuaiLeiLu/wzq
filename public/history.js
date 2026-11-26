const dialog = document.querySelector('#history-dialog');
const form = document.querySelector('#history-filter');
const content = document.querySelector('#history-content');
const status = document.querySelector('#history-status');
const back = document.querySelector('#history-back');
const prev = document.querySelector('#history-prev');
const next = document.querySelector('#history-next');
const clock = new Intl.DateTimeFormat('zh-CN', { timeZone: 'Asia/Shanghai', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
const gameName = game => game === 'ddz' ? '斗地主' : '五子棋';
const stateNames = { active: '进行中', interrupted: '已中止', finished: '已结束', legacy: '旧版计分记录' };
const ranks = ['3','4','5','6','7','8','9','10','J','Q','K','A','2'];
const cardName = card => card >= 52 ? ['小王','大王'][card - 52] : '♠♥♣♦'[card % 4] + ranks[Math.floor(card / 4)];
const square = index => String.fromCharCode(65 + index % 15) + (Math.floor(index / 15) + 1);
let filters, pages, cursor, matchId, request;

function element(tag, text, className) {
  const node = document.createElement(tag);
  node.textContent = text;
  if (className) node.className = className;
  return node;
}

function heading(match) {
  return [gameName(match.game), match.room && '房间 ' + match.room, match.result || stateNames[match.status]].filter(Boolean).join(' · ');
}

function eventText(event, match) {
  const d = event.detail || {};
  const player = event.seat == null ? '' : match.players[event.seat];
  const color = player && match.game === 'gomoku' ? ['黑棋','白棋'][event.seat] : '';
  const parts = [color, player, event.action];
  if (d.message) parts.push(d.message);
  if (Number.isInteger(d.index)) parts.push(square(d.index));
  if (d.cards) parts.push(d.cards.map(cardName).join(' '));
  if (Number.isInteger(d.left)) parts.push('剩 ' + d.left + ' 张');
  if (d.reason) parts.push(({ line:'五子连线', timeout:'落子超时', left:'棋手离开', draw:'棋盘已满' })[d.reason]);
  if (d.line?.length) parts.push(d.line.map(square).join('、'));
  if (Number.isInteger(d.winnerSeat)) parts.push(match.players[d.winnerSeat] + ' 先出完手牌');
  if (d.multiplier) parts.push(d.multiplier + ' 倍');
  if (d.spring) parts.push(d.spring === 'spring' ? '春天' : '反春');
  if (d.counts) parts.push(match.players.map((name, seat) => name + ' 剩 ' + d.counts[seat] + ' 张').join('，'));
  if (d.deltas) parts.push('结算分 ' + d.deltas.map(n => n > 0 ? '+' + n : n).join(' / '));
  return parts.filter(Boolean).join(' · ');
}

function detail(match) {
  content.append(element('h3', heading(match)));
  content.append(element('p', [match.day, match.started && clock.format(match.started), match.mode, match.players.join(' / ')].filter(Boolean).join(' · ')));
  content.append(element('small', '对局编号 ' + match.id));
  if (match.status === 'legacy') content.append(element('p', '旧版只保存了计分日期和编号。'));
  for (const event of match.events) {
    const line = element('div', '', 'history-event');
    line.append(element('time', clock.format(event.at_ms)), document.createTextNode(eventText(event, match)));
    content.append(line);
  }
}

function list(data) {
  cursor = data.next;
  next.hidden = !cursor;
  if (!data.matches.length) status.textContent = '没有找到符合条件的对局。';
  for (const match of data.matches) {
    const row = element('button', '', 'history-row');
    row.type = 'button';
    row.append(element('strong', heading(match)), element('small', [match.day, match.started && clock.format(match.started), match.players.join(' / '), match.mode].filter(Boolean).join(' · ')));
    row.addEventListener('click', () => { matchId = match.id; load(); });
    content.append(row);
  }
}

async function load() {
  request?.abort();
  const current = new AbortController();
  request = current;
  back.hidden = !matchId;
  prev.hidden = Boolean(matchId) || pages.length < 2;
  next.hidden = true;
  form.hidden = Boolean(matchId);
  status.textContent = '正在读取对局记录…';
  content.replaceChildren();
  const params = new URLSearchParams(filters);
  if (pages.at(-1)) params.set('before', pages.at(-1));
  try {
    const url = matchId ? '/api/history/' + matchId : '/api/history?' + params;
    const response = await fetch(url, {signal: current.signal});
    if (!response.ok) throw new Error('读取失败');
    const data = await response.json();
    if (request !== current || !dialog.open) return;
    status.textContent = '';
    if (matchId) detail(data); else list(data);
  } catch {
    if (!current.signal.aborted) status.textContent = '暂时无法读取对局记录，可点击刷新重试。';
  }
}

function search() {
  filters = [...new FormData(form)].filter(([, value]) => value);
  matchId = null;
  pages = [null];
  load();
}

export function openHistory(game, room) {
  form.reset();
  form.elements.room.value = room || '';
  form.elements.game.value = ['ddz','gomoku'].includes(game) ? game : '';
  dialog.showModal();
  search();
}

form.addEventListener('submit', event => { event.preventDefault(); search(); });
prev.addEventListener('click', () => { pages.pop(); load(); });
next.addEventListener('click', () => { if (cursor) { pages.push(cursor); load(); } });
back.addEventListener('click', () => { matchId = null; load(); });
document.querySelector('#history-refresh').addEventListener('click', load);
document.querySelector('#history-close').addEventListener('click', () => dialog.close());
dialog.addEventListener('close', () => request?.abort());
