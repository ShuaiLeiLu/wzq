// 弈间 · 斗地主: lobby, table, hand selection, chat and result.
// app.js owns the connection and passes shared helpers in through createDdz().

import { createFx, playWords } from './ddz-fx.js';

const RANKS = ['3', '4', '5', '6', '7', '8', '9', '10', 'J', 'Q', 'K', 'A', '2', '小王', '大王'];
const SUITS = ['♠', '♥', '♣', '♦'];
const SUIT_NAMES = ['黑桃', '红桃', '梅花', '方块'];
const PLAY_MS = 25_000;
const DEAL_STEP_MS = 40;
const BID_MS = 15_000;
const EMOTES = ['😄', '👍', '😂', '😮', '😭', '🎉', '🔥', '🍵'];
const PHRASES = ['来了来了，马上出！', '这手牌，稳了。', '好牌！佩服佩服。', '炸得漂亮！', '队友，靠你了！', '别急，让我想想～', '不好意思，又赢啦。', '再来一局？'];
const COUNTER_KEY = 'yijian-ddz-counter';
const BARRAGE_KEY = 'yijian-ddz-barrage';

const rankOf = card => (card < 52 ? card >> 2 : card - 52 + 13);

function cardName(card) {
  if (card >= 52) return card === 53 ? '大王' : '小王';
  return `${SUIT_NAMES[card % 4]} ${RANKS[card >> 2]}`;
}

function cardClass(card) {
  if (card === 53) return 'joker big';
  if (card === 52) return 'joker';
  return card % 4 === 1 || card % 4 === 3 ? 'red' : '';
}

function cardFace(card) {
  if (card >= 52) return `<b>${card === 53 ? '大王' : '小王'}</b><i>★</i>`;
  return `<b>${RANKS[card >> 2]}</b><i>${SUITS[card % 4]}</i>`;
}

function miniCards(cards, extra = '') {
  return `<span class="mini-cards ${extra}">${cards.map(card => `<span class="pcard mini ${cardClass(card)}" title="${cardName(card)}">${cardFace(card)}</span>`).join('')}</span>`;
}

export function createDdz(ctx) {
  const { $, run, toast, escapeHTML, icon, tone, initial, avatarInner, scoreText, setText, setHTML } = ctx;
  const el = {
    lobby: $('#ddz-lobby'),
    view: $('#ddz-view'),
    list: $('#table-list'),
    search: $('#table-search'),
    left: $('#ddz-left'),
    right: $('#ddz-right'),
    playLeft: $('#ddz-play-left'),
    playRight: $('#ddz-play-right'),
    playSelf: $('#ddz-play-self'),
    bottom: $('#ddz-bottom'),
    message: $('#ddz-message'),
    self: $('#ddz-self'),
    actions: $('#ddz-actions'),
    hand: $('#ddz-hand'),
    counter: $('#ddz-counter'),
    barrage: $('#ddz-barrage'),
    chatLog: $('#ddz-chat-log'),
    chatInput: $('#ddz-chat-input'),
    quick: $('#ddz-quick-talk'),
    result: $('#ddz-result-dialog'),
    joinCode: $('#ddz-join-code'),
    joinError: $('#ddz-join-error')
  };
  let filter = 'all';
  let rendered = '';
  let selected = new Set();
  let handKey = '';
  let hintKey = '';
  let hintIndex = -1;
  let drag = null;
  let lastKey = null;
  let sideTimer = null;
  let seen = null;
  let lastTick = null;
  let wasDealing = null;
  const barrageQueue = [];
  const fx = createFx();
  const felt = $('#felt');
  const banner = $('#ddz-banner');

  const table = () => ctx.state()?.table;
  const game = () => table()?.game;
  const mySeat = () => table()?.seat ?? -1;
  const remaining = deadline => Math.max(0, deadline - ctx.now());

  /* ---------- Lobby ---------- */

  function tableThumb(room) {
    const cards = [0, 1, 2].map(index => `<span class="thumb-seat ${index < room.humans ? 'human' : index < room.count ? 'bot' : ''}"></span>`).join('');
    return `<div class="room-thumb table-thumb" aria-hidden="true"><div class="felt-mini">${cards}${room.playing ? '<span class="thumb-play">♠</span>' : ''}</div></div>`;
  }

  function tableCard(room) {
    const status = room.playing ? '对局中' : !room.joinable ? '已满员' : room.humans ? `差 ${3 - room.count} 人` : '空桌';
    const chipTone = room.playing ? 'live' : room.joinable && room.humans ? 'wait' : '';
    const note = room.playing ? '牌局进行中，可入场观战'
      : room.bots ? `${room.humans} 位玩家 · ${room.bots} 个机器人，入座可替换机器人`
        : room.humans ? `${room.humans} 位玩家等待牌友` : '虚位以待，入座即可开局';
    const dots = [0, 1, 2].map(index => `<span class="seat-dot ${index < room.humans ? 'human' : index < room.count ? 'bot' : ''}"></span>`).join('');
    const disabled = ctx.connected() ? '' : 'disabled';
    const watch = room.watchable ? `<button class="btn btn-line" type="button" data-action="watch" data-id="${room.id}" ${disabled}>${icon('eye')}观战</button>` : '';
    const join = room.joinable
      ? `<button class="btn btn-gold-flat" type="button" data-action="join" data-id="${room.id}" ${disabled}>入座</button>`
      : room.watchable ? '' : '<button class="btn btn-line" type="button" disabled>暂不可加入</button>';
    return `<article class="room-card${room.joinable ? ' open' : ''}">
      ${tableThumb(room)}
      <div class="room-main">
        <div class="room-title"><h3 title="${escapeHTML(room.name)}">${escapeHTML(room.name)}</h3><span class="room-no">#${room.id}</span><span class="chip ${chipTone}"><i></i>${status}</span></div>
        <div class="room-seats"><span><span class="seat-dots" aria-hidden="true">${dots}</span>${room.count} / 3 入座</span><span class="vacant">${note}</span></div>
        <div class="room-foot"><span>三人斗地主 · 每手 25 秒</span><div class="room-actions">${watch}${join}</div></div>
      </div>
    </article>`;
  }

  function renderLobby() {
    const state = ctx.state();
    if (!state) return;
    const tables = state.tables;
    const playing = tables.filter(room => room.playing).length;
    setText($('#ddz-stat-online'), String(state.online));
    setText($('#ddz-stat-playing'), String(playing));
    setText($('#ddz-hero-chip-title'), playing ? `${playing} 桌正在斗地主` : '牌桌已备好');
    setText($('#ddz-hero-chip-note'), playing ? '点「观战」看看高手怎么打' : '三缺一？机器人随时补位');
    const query = el.search.value.trim().toLowerCase().replace(/^#/, '');
    const matches = tables.filter(room => (filter === 'all' || (filter === 'open' ? room.joinable : room.watchable))
      && (room.name.toLowerCase().includes(query) || room.id.includes(query)));
    setText($('#table-count'), matches.length === tables.length ? `${tables.length} 张牌桌 · ${playing} 桌对局中` : `显示 ${matches.length} / ${tables.length} 张`);
    const signature = JSON.stringify([ctx.connected(), matches]);
    if (signature === rendered) return;
    rendered = signature;
    el.list.setAttribute('aria-busy', 'false');
    el.list.innerHTML = matches.length ? matches.map(tableCard).join('') : '<p class="empty">没有符合条件的牌桌，换个筛选条件，或者自己开一桌。</p>';
  }

  async function fetchRanking() {
    const response = await fetch('/api/ddz/leaderboard', { signal: AbortSignal.timeout(7000) });
    if (!response.ok) throw new Error('Ranking unavailable');
    return response.json();
  }

  function rankRows(entries) {
    return entries.slice(0, 5).map(row => `<li class="${row.rank === 1 ? 'top' : ''}">
      <span class="medal ${row.rank <= 3 ? `medal-${row.rank}` : ''}">${row.rank}</span>
      <span class="avatar ${tone(row.name)}">${escapeHTML(initial(row.name))}</span>
      <span class="rank-who"><b>${escapeHTML(row.name)}</b><small>${row.games} 局 · ${row.wins} 胜</small></span>
      <span class="points${row.points < 0 ? ' negative' : ''}">${scoreText(row.points)}</span>
    </li>`).join('');
  }

  async function refreshSide(listId, ownId) {
    const list = $(listId);
    try {
      const ranking = await fetchRanking();
      setHTML(list, rankRows(ranking.entries) || '<li class="rank-empty">今天还没有人上榜，下一局就是你的。</li>');
      if (ownId) {
        const account = ctx.state()?.me?.account;
        setHTML($(ownId), !account ? '登录后三人真人局计入排行'
          : ranking.you ? `你今日第 <b>${ranking.you.rank}</b> 名 · ${scoreText(ranking.you.points)} 分` : '今天还没有计分牌局');
      }
    } catch {
      setHTML(list, '<li class="rank-empty">暂时无法读取排行榜。</li>');
    }
  }

  function watchSide(active) {
    if (active && !sideTimer) {
      refreshSide('#ddz-side-rank-list', '#ddz-side-rank-own');
      sideTimer = setInterval(() => { if (!document.hidden) refreshSide('#ddz-side-rank-list', '#ddz-side-rank-own'); }, 30_000);
    } else if (!active && sideTimer) {
      clearInterval(sideTimer);
      sideTimer = null;
    }
  }

  /* ---------- Table ---------- */

  function roleOf(room, seat) {
    const landlord = room.game?.landlord;
    if (landlord === null || landlord === undefined) return '';
    return landlord === seat ? '地主' : '农民';
  }

  function seatStatus(room, seat) {
    const person = room.seats[seat];
    const current = room.game;
    if (!person) return ['empty', '空位'];
    if (person.departed) return ['off', '已离开 · 托管'];
    if (!person.online) return ['off', '重连中'];
    if (current?.phase && current.phase !== 'finished' && current.trustee[seat]) return ['warn', '托管中'];
    if (person.bot) return ['bot', '机器人'];
    if (!current || current.phase === 'finished') return person.ready ? ['on', '已准备'] : ['', '未准备'];
    return ['', ''];
  }

  function isActive(current, seat) {
    if (!current || current.phase === 'finished') return false;
    if (current.phase === 'doubling') return current.doubled[seat] === null;
    return current.turn === seat;
  }

  function clockHTML(current, seat) {
    if (!isActive(current, seat)) return '';
    return `<span class="turn-clock" data-clock="${seat}"><svg viewBox="0 0 36 36" aria-hidden="true"><circle cx="18" cy="18" r="15.5"/><circle class="ring" cx="18" cy="18" r="15.5" pathLength="100"/></svg><b></b></span>`;
  }

  function opponentHTML(room, seat, side) {
    const person = room.seats[seat];
    const current = room.game;
    if (!person) {
      const canFill = room.seat !== -1 && (!current || current.phase === 'finished') && ctx.connected();
      return `<div class="opp vacant">
        <span class="avatar avatar-lg">${icon('user')}</span>
        <div class="opp-id"><b>等待入座</b><small>${room.practice ? '' : '复制邀请发给牌友'}</small></div>
        ${canFill ? `<button class="btn btn-line btn-sm" type="button" data-action="ddz-bot">${icon('bot')}补机器人</button>` : ''}
      </div>`;
    }
    const role = roleOf(room, seat);
    const [statusTone, status] = seatStatus(room, seat);
    const count = current?.counts[seat] ?? 0;
    const alarm = current?.phase === 'playing' && count > 0 && count <= 2;
    const hand = current?.hands[seat];
    const face = person.bot ? icon('bot') : avatarInner(person.name, person.avatar);
    return `<div class="opp${isActive(current, seat) ? ' active' : ''}" data-side="${side}">
      <div class="opp-face">
        <span class="avatar avatar-lg ${person.bot ? 'bot-face' : tone(person.name)}">${face}</span>
        ${role ? `<span class="role-hat${role === '地主' ? ' landlord' : ''}">${role}</span>` : ''}
        ${clockHTML(current, seat)}
      </div>
      <div class="opp-id"><b title="${escapeHTML(person.name)}">${escapeHTML(person.name)}</b>${status ? `<small class="state ${statusTone}">${status}</small>` : ''}</div>
      ${current ? `<div class="opp-count${alarm ? ' alarm' : ''}"><span class="card-back" aria-hidden="true"></span><b>${count}</b><small>${alarm ? (count === 1 ? '报单！' : '报双！') : '张'}</small></div>` : ''}
      ${hand && hand.length && seat !== room.seat ? miniCards(hand, 'revealed') : ''}
    </div>`;
  }

  function actHTML(current, seat) {
    const act = current?.acts?.[seat];
    if (!act) return '';
    if (act.cards.length) {
      const label = act.kind === 'bomb' || act.kind === 'rocket' ? ` <em class="boom">${act.text}</em>` : '';
      return `<div class="act act-cards" data-key="${act.cards.join('-')}">${miniCards(act.cards)}${label}</div>`;
    }
    const tone = ['不出', '不叫', '不抢', '不加倍'].includes(act.text) ? 'quiet' : 'loud';
    return `<div class="act act-say ${tone}">${act.text}</div>`;
  }

  function tableMessage(room) {
    const current = room.game;
    const seat = room.seat;
    const names = room.seats.map(person => person?.name ?? '空位');
    const filled = room.seats.filter(Boolean).length;
    if (!ctx.connected()) return '正在重连牌桌…';
    if (!current || current.phase === 'finished') {
      if (filled < 3) return `等待玩家入座（${filled} / 3）`;
      if (seat === -1) return '等待玩家准备';
      return room.seats[seat]?.ready ? '你已准备，等待其他玩家' : '三人到齐，点「准备」开局';
    }
    const turn = current.turn;
    if (current.phase === 'bidding') {
      const verb = current.robbing ? '抢地主' : '叫地主';
      return turn === seat ? `轮到你${verb}` : `${names[turn]} 正在考虑${verb}`;
    }
    if (current.phase === 'doubling') {
      return seat !== -1 && current.doubled[seat] === null ? '选择是否加倍' : '等待其他玩家选择加倍';
    }
    if (turn === seat) return current.lead ? '轮到你出牌' : '轮到你，压过上家或不出';
    return `${names[turn]} 出牌中…`;
  }

  function actionsHTML(room) {
    const current = room.game;
    const seat = room.seat;
    const live = ctx.connected();
    const d = live ? '' : 'disabled';
    if (seat === -1) {
      const canSit = (!current || current.phase === 'finished') && room.seats.some(person => !person || person.bot) && !room.practice;
      return canSit ? `<button class="btn btn-gold" type="button" data-action="ddz-take-seat" ${d}>${icon('door')}入座</button>` : '';
    }
    if (!current || current.phase === 'finished') {
      const me = room.seats[seat];
      const empty = room.seats.some(person => !person);
      return `${empty ? `<button class="btn btn-line" type="button" data-action="ddz-bot" ${d}>${icon('bot')}补机器人</button>` : ''}
        <button class="btn ${me?.ready ? 'btn-line' : 'btn-gold'} btn-wide" type="button" data-action="ddz-ready" ${d}>${icon('check')}${me?.ready ? '取消准备' : current ? '再来一局' : '准备'}</button>`;
    }
    const trustee = current.trustee[seat];
    const trusteeButton = `<button class="btn btn-quiet" type="button" data-action="ddz-trustee" data-yes="${!trustee}" ${d}>${icon('bot')}${trustee ? '取消托管' : '托管'}</button>`;
    const myTurn = current.turn === seat;
    if (current.phase === 'bidding') {
      if (!myTurn) return trusteeButton;
      const [yes, no] = current.robbing ? ['抢地主', '不抢'] : ['叫地主', '不叫'];
      return `<button class="btn btn-line" type="button" data-action="ddz-bid" data-yes="false" ${d}>${no}</button>
        <button class="btn btn-gold btn-wide" type="button" data-action="ddz-bid" data-yes="true" ${d}>${yes}</button>${trusteeButton}`;
    }
    const canShow = current.landlord === seat && !current.shown && current.played.length === 0;
    const show = canShow ? `<button class="btn btn-ink" type="button" data-action="ddz-show" ${d}>${icon('eye')}明牌 ×2</button>` : '';
    if (current.phase === 'doubling') {
      if (current.doubled[seat] !== null) return `${show}${trusteeButton}`;
      return `<button class="btn btn-line" type="button" data-action="ddz-double" data-yes="false" ${d}>不加倍</button>
        <button class="btn btn-gold btn-wide" type="button" data-action="ddz-double" data-yes="true" ${d}>加倍 ×2</button>${show}`;
    }
    if (!myTurn) return `${show}${trusteeButton}`;
    const noHint = current.hints.length === 0;
    return `${current.lead ? '' : `<button class="btn btn-line" type="button" data-action="ddz-pass" ${d}>不出</button>`}
      <button class="btn btn-line" type="button" data-action="ddz-hint" ${d} ${noHint ? 'disabled' : ''}>${icon('bulb')}${noHint ? '要不起' : '提示'}</button>
      <button class="btn btn-gold btn-wide" type="button" data-action="ddz-play" ${d} ${selected.size ? '' : 'disabled'}>出牌</button>
      ${show}${trusteeButton}`;
  }

  function selfHTML(room) {
    const seat = room.seat === -1 ? 0 : room.seat;
    const person = room.seats[seat];
    const current = room.game;
    if (!person) return '';
    const role = roleOf(room, seat);
    const [statusTone, status] = seatStatus(room, seat);
    const doubled = current?.doubled?.[seat];
    return `<div class="self${isActive(current, seat) ? ' active' : ''}">
      <div class="opp-face">
        <span class="avatar avatar-sm ${person.bot ? 'bot-face' : tone(person.name)}">${person.bot ? icon('bot') : avatarInner(person.name, person.avatar)}</span>
        ${clockHTML(current, seat)}
      </div>
      <b title="${escapeHTML(person.name)}">${escapeHTML(person.name)}${room.seat === -1 ? '' : ' <small>（你）</small>'}</b>
      ${role ? `<span class="role-tag${role === '地主' ? ' landlord' : ''}">${role}</span>` : ''}
      ${doubled ? '<span class="role-tag">已加倍</span>' : ''}
      ${status ? `<small class="state ${statusTone}">${status}</small>` : ''}
      ${current ? `<span class="self-count">剩 ${current.counts[seat]} 张</span>` : ''}
    </div>`;
  }

  function renderHand(room) {
    const current = room.game;
    const seat = room.seat === -1 ? 0 : room.seat;
    const cards = current?.hands[seat];
    const mine = room.seat !== -1 && current?.phase && current.phase !== 'finished';
    if (!cards) {
      const count = current?.counts[seat] ?? 0;
      const key = `backs:${count}`;
      if (handKey !== key) {
        handKey = key;
        el.hand.style.setProperty('--n', count);
        el.hand.innerHTML = Array.from({ length: count }, () => '<span class="pcard back"></span>').join('');
      }
      el.hand.classList.remove('mine');
      return;
    }
    for (const card of [...selected]) if (!cards.includes(card)) selected.delete(card);
    const key = `${cards.join(',')}|${mine}`;
    el.hand.classList.toggle('mine', Boolean(mine));
    if (handKey !== key) {
      handKey = key;
      el.hand.style.setProperty('--n', cards.length);
      el.hand.innerHTML = cards.map(card => mine
        ? `<button type="button" class="pcard ${cardClass(card)}" data-card="${card}" aria-label="${cardName(card)}" aria-pressed="false">${cardFace(card)}</button>`
        : `<span class="pcard ${cardClass(card)}" title="${cardName(card)}">${cardFace(card)}</span>`).join('');
    }
    for (const node of el.hand.querySelectorAll('[data-card]')) {
      const on = selected.has(Number(node.dataset.card));
      node.classList.toggle('up', on);
      node.setAttribute('aria-pressed', String(on));
    }
  }

  function renderCounter(room) {
    const current = room.game;
    const on = $('#ddz-counter-toggle').getAttribute('aria-pressed') === 'true';
    el.counter.hidden = !on || current?.phase !== 'playing';
    if (el.counter.hidden) return;
    const left = Array.from({ length: 15 }, (_, rank) => (rank >= 13 ? 1 : 4));
    const own = room.seat === -1 ? [] : current.hands[room.seat] ?? [];
    for (const card of [...current.played, ...own]) left[rankOf(card)] -= 1;
    const cells = [14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0].map(rank => `<span class="${left[rank] === 0 ? 'gone' : ''}"><b>${rank === 14 ? '大' : rank === 13 ? '小' : RANKS[rank]}</b><i>${left[rank]}</i></span>`).join('');
    setHTML(el.counter, `<small>记牌器</small>${cells}`);
  }

  function renderBottom(current) {
    if (!current) {
      setHTML(el.bottom, '');
      return;
    }
    const html = current.bottom.length
      ? miniCards(current.bottom, 'bottom')
      : '<span class="mini-cards bottom"><span class="pcard mini back"></span><span class="pcard mini back"></span><span class="pcard mini back"></span></span>';
    setHTML(el.bottom, `${html}<small>底牌</small>`);
  }

  function renderTable(room) {
    const current = room.game;
    const watching = room.seat === -1;
    const me = watching ? 0 : room.seat;
    const right = (me + 1) % 3;
    const left = (me + 2) % 3;
    const playing = Boolean(current && current.phase !== 'finished');

    setText($('#ddz-title'), room.name);
    setText($('#ddz-code'), `#${room.id}`);
    const allHuman = room.seats.every(person => person && !person.bot);
    setText($('#ddz-facts'), room.practice ? '人机练习 · 不计排行' : allHuman ? '三人斗地主 · 叫抢加倍 · 每手 25 秒' : '三人斗地主 · 有机器人的局不计排行');
    $('#ddz-watch-badge').hidden = !watching;
    $('#ddz-watch-invite').hidden = watching || room.practice;
    $('#ddz-invite').hidden = room.practice;
    setText($('#ddz-invite span'), watching ? '复制观战链接' : '复制邀请');
    setText($('#ddz-spectators span'), `${room.spectatorCount} 人观战`);

    const chip = $('#ddz-state');
    const phaseNames = { bidding: '叫地主', doubling: '加倍', playing: '出牌中', finished: '本局结束' };
    chip.className = `chip ${!ctx.connected() ? 'alert' : playing ? 'live' : 'wait'}`;
    setText(chip.querySelector('span'), !ctx.connected() ? '正在重连' : current ? phaseNames[current.phase] : '等待开局');
    setHTML($('#ddz-multiplier'), `倍数 <b>×${current?.multiplier ?? 1}</b>`);
    const bombs = current?.bombs ?? 0;
    setText($('#ddz-note'), current ? `抢地主 ${current.robs} 次 · 炸弹 ${bombs} 个${current.shown ? ' · 已明牌' : ''}` : '');

    setHTML(el.left, opponentHTML(room, left, 'left'));
    setHTML(el.right, opponentHTML(room, right, 'right'));
    setHTML(el.playLeft, actHTML(current, left));
    setHTML(el.playRight, actHTML(current, right));
    setHTML(el.playSelf, actHTML(current, me));
    renderBottom(current);
    setText(el.message, tableMessage(room));
    el.message.classList.toggle('go', !watching && playing && isActive(current, me));
    setHTML(el.self, selfHTML(room));

    const turnKey = current ? `${current.phase}:${current.turn}:${current.played.length}` : '';
    if (turnKey !== hintKey) {
      hintKey = turnKey;
      hintIndex = -1;
    }
    renderHand(room);
    setHTML(el.actions, actionsHTML(room));
    renderCounter(room);
    renderChat(room);
    renderResult(room);
    notice(room);
    wasDealing = null;
    renderClock();
  }

  /* ---------- Effects ---------- */

  function showBanner(text, tone = '') {
    banner.textContent = text;
    banner.className = `fx-banner ${tone}`;
    void banner.offsetWidth;
    banner.classList.add('show');
  }

  function shake() {
    felt.classList.remove('shake');
    void felt.offsetWidth;
    felt.classList.add('shake');
  }

  function animateDeal() {
    requestAnimationFrame(() => {
      const box = felt.getBoundingClientRect();
      const deck = el.bottom.getBoundingClientRect();
      const ox = deck.left + deck.width / 2;
      const oy = deck.top + deck.height / 2;
      // Cards go round the table one at a time, starting with this seat.
      [...el.hand.querySelectorAll('.pcard')].forEach((node, index) => {
        const rect = node.getBoundingClientRect();
        node.style.setProperty('--fx', `${ox - rect.left - rect.width / 2}px`);
        node.style.setProperty('--fy', `${oy - rect.top - rect.height / 2}px`);
        node.style.animationDelay = `${index * 3 * DEAL_STEP_MS}ms`;
        node.classList.add('dealing');
        node.addEventListener('animationend', () => {
          node.classList.remove('dealing');
          node.style.animationDelay = '';
        }, { once: true });
      });
      for (const [side, offset] of [[el.right, 1], [el.left, 2]]) {
        const target = (side.querySelector('.opp-count') ?? side).getBoundingClientRect();
        for (let index = 0; index < 17; index++) {
          const ghost = document.createElement('span');
          ghost.className = 'pcard mini back ghost';
          ghost.style.left = `${ox - box.left}px`;
          ghost.style.top = `${oy - box.top}px`;
          ghost.style.setProperty('--tx', `${target.left + target.width / 2 - ox}px`);
          ghost.style.setProperty('--ty', `${target.top + target.height / 2 - oy}px`);
          ghost.style.animationDelay = `${(index * 3 + offset) * DEAL_STEP_MS}ms`;
          ghost.addEventListener('animationend', () => ghost.remove(), { once: true });
          felt.append(ghost);
        }
      }
      el.bottom.classList.remove('dealt');
      void el.bottom.offsetWidth;
      el.bottom.classList.add('dealt');
      setTimeout(() => el.bottom.classList.remove('dealt'), 51 * DEAL_STEP_MS + 500);
      for (let index = 0; index < 51; index++) setTimeout(() => fx.play('deal'), index * DEAL_STEP_MS);
    });
  }

  function actEffect(act) {
    if (act.cards.length) {
      if (act.kind === 'rocket') {
        fx.play('rocket');
        shake();
        showBanner('王炸', 'rocket');
      } else if (act.kind === 'bomb') {
        fx.play('bomb');
        shake();
        showBanner('炸弹', 'bomb');
      } else {
        fx.play('play');
        if (act.kind.startsWith('plane')) showBanner('飞机', 'plane');
      }
    } else {
      fx.play(['叫地主', '抢地主', '加倍'].includes(act.text) ? 'bid' : act.text === '不出' ? 'pass' : 'decline');
    }
    fx.say(playWords(act));
  }

  /** Sounds and effects for whatever changed since the last update. */
  function notice(room) {
    const current = room.game;
    if (!current) {
      seen = { room: room.id };
      return;
    }
    const acts = current.acts.map(act => (act ? `${act.text}|${act.cards.join(',')}` : ''));
    const seat = room.seat;
    const myTurn = seat !== -1 && isActive(current, seat);
    const fresh = seen?.room !== room.id;
    if (fresh || current.deal !== seen.deal) {
      // A brand-new deal (not a reconnect mid-bidding) is dealt out on screen.
      if (current.phase === 'bidding' && current.acts.every(act => !act)) animateDeal();
      seen = { room: room.id, deal: current.deal, acts, landlord: current.landlord, phase: current.phase, myTurn, alarms: new Set() };
      return;
    }
    acts.forEach((key, index) => {
      if (key && key !== seen.acts[index]) actEffect(current.acts[index]);
    });
    if (current.landlord !== null && seen.landlord === null) {
      setTimeout(() => fx.play('landlord'), 250);
    }
    if (current.phase === 'playing') {
      current.counts.forEach((count, index) => {
        const key = `${index}:${count}`;
        if (count > 0 && count <= 2 && !seen.alarms.has(key)) {
          seen.alarms.add(key);
          setTimeout(() => fx.say(count === 1 ? '我只剩一张牌了' : '我只剩两张牌了'), 900);
        }
      });
    }
    if (current.phase === 'finished' && seen.phase !== 'finished') {
      const outcome = current.outcome;
      const won = seat === -1 ? true : outcome.deltas[seat] > 0;
      setTimeout(() => fx.play(won ? 'win' : 'lose'), 500);
      if (outcome.spring) {
        fx.play('spring');
        showBanner(outcome.spring === 'spring' ? '春天' : '反春', 'spring');
      }
    }
    if (myTurn && !seen.myTurn && current.phase !== 'finished') fx.play('turn');
    seen = { ...seen, acts, landlord: current.landlord, phase: current.phase, myTurn };
  }

  function renderClock() {
    const current = game();
    if (!current || current.phase === 'finished') {
      el.actions.hidden = false;
      return;
    }
    const limit = current.phase === 'playing' ? PLAY_MS : BID_MS;
    // Bidding waits for the deal animation; until then there is nothing to do.
    const dealing = current.phase === 'bidding' && remaining(current.deadline) > BID_MS;
    if (dealing !== wasDealing) {
      wasDealing = dealing;
      setText(el.message, dealing ? '正在发牌…' : tableMessage(table()));
      el.actions.hidden = dealing;
      for (const node of document.querySelectorAll('#ddz-view [data-clock]')) node.hidden = dealing;
    }
    const left = Math.min(limit, remaining(current.deadline));
    const seconds = Math.ceil(left / 1000);
    const mine = mySeat() !== -1 && isActive(current, mySeat());
    if (mine && seconds <= 5 && seconds > 0 && seconds !== lastTick) fx.play('tick');
    lastTick = mine ? seconds : null;
    for (const node of document.querySelectorAll('#ddz-view [data-clock]')) {
      setText(node.querySelector('b'), String(seconds));
      node.querySelector('.ring').style.strokeDashoffset = String(100 - Math.min(100, (left / limit) * 100));
      node.classList.toggle('low', seconds <= 5);
    }
  }

  /* ---------- Chat & barrage ---------- */

  function roleLabel(room, message) {
    if (message.seat === -1) return '观众';
    const role = roleOf(room, message.seat);
    if (room.seats[message.seat]?.bot) return role || '机器人';
    return role || '玩家';
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
    $('#ddz-chat-empty').hidden = Boolean(guidance.length);
    const incoming = guidance.filter(message => message.id > latest);
    const myName = ctx.state().me.name;
    const isSelf = message => (room.seat !== -1 ? message.seat === room.seat : message.seat === -1 && message.name === myName);
    for (const message of incoming) {
      if (message.seat === -2) {
        el.chatLog.insertAdjacentHTML('beforeend', `<p class="msg-system" data-id="${message.id}">${escapeHTML(message.text)}</p>`);
        continue;
      }
      el.chatLog.insertAdjacentHTML('beforeend', `<div class="msg${isSelf(message) ? ' self' : ''}" data-id="${message.id}">
        <span class="avatar ${tone(message.name)}" aria-hidden="true">${escapeHTML(initial(message.name))}</span>
        <div class="msg-body"><div class="msg-meta"><b>${escapeHTML(message.name)}</b><span class="role${message.seat === -1 ? ' viewer' : ''}">${roleLabel(room, message)}</span></div><p>${escapeHTML(message.text)}</p></div>
      </div>`);
    }
    if (incoming.length && !entering && $('#ddz-barrage-toggle').getAttribute('aria-pressed') === 'true') {
      barrageQueue.push(...incoming.filter(message => message.seat !== -2).slice(-6).map(message => ({ ...message, self: isSelf(message) })));
      if (barrageQueue.length > 6) barrageQueue.splice(0, barrageQueue.length - 6);
      showBarrage();
    }
    while (el.chatLog.children.length > guidance.length) el.chatLog.firstElementChild.remove();
    if (follow && guidance.at(-1)?.id > latest) el.chatLog.scrollTop = el.chatLog.scrollHeight;
    el.chatInput.disabled = !ctx.connected();
    setText($('#ddz-chat-jump'), guidance.length ? `对话 ${guidance.length}` : '对话');
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
    const current = room.game;
    const key = current ? `${room.id}:${current.phase}:${current.played.length}` : `${room.id}:none`;
    const previous = lastKey;
    lastKey = key;
    const justFinished = current?.phase === 'finished' && previous !== key && previous?.startsWith(`${room.id}:playing:`);
    if (!justFinished) {
      if (current && current.phase !== 'finished' && el.result.open) el.result.close();
      return;
    }
    const outcome = current.outcome;
    const watching = room.seat === -1;
    const delta = watching ? 0 : outcome.deltas[room.seat];
    const won = !watching && delta > 0;
    const seal = $('#ddz-result-seal');
    seal.className = `result-seal${!watching && !won ? ' lose' : ''}`;
    seal.textContent = watching ? (outcome.landlordWon ? '地' : '农') : won ? '胜' : '负';
    const side = outcome.landlordWon ? '地主获胜' : '农民获胜';
    const spring = outcome.spring === 'spring' ? ' · 春天' : outcome.spring === 'antiSpring' ? ' · 反春' : '';
    setText($('#ddz-result-title'), `${side}${spring}`);
    setText($('#ddz-result-copy'), watching ? '本局已结束，可以留下观看下一局。'
      : won ? `漂亮！本局 ${scoreText(delta)} 分。` : `本局 ${scoreText(delta)} 分，下一局扳回来。`);
    $('#ddz-result-rows').innerHTML = room.seats.map((person, seat) => `<tr class="${seat === room.seat ? 'me' : ''}">
      <th scope="row">${escapeHTML(person?.name ?? '空位')}</th>
      <td><span class="role-tag${seat === current.landlord ? ' landlord' : ''}">${seat === current.landlord ? '地主' : '农民'}</span></td>
      <td>${current.doubled[seat] ? '×2' : '—'}</td>
      <td class="points${outcome.deltas[seat] < 0 ? ' negative' : ''}">${scoreText(outcome.deltas[seat])}</td>
    </tr>`).join('');
    const factors = [`公共倍数 ×${outcome.multiplier}`];
    if (current.robs) factors.push(`抢地主 ${current.robs} 次`);
    if (current.bombs) factors.push(`炸弹 ${current.bombs} 个`);
    if (current.shown) factors.push('明牌');
    if (outcome.spring) factors.push(outcome.spring === 'spring' ? '春天' : '反春');
    factors.push(room.ranked || allRankable(room) ? '单人封顶 64 分' : '本局不计排行');
    setText($('#ddz-result-factors'), factors.join(' · '));
    $('#ddz-result-again').hidden = watching;
    if (!ctx.anyDialogOpen()) el.result.showModal();
  }

  function allRankable(room) {
    return !room.practice && room.seats.every(person => person && !person.bot);
  }

  /* ---------- Hand selection ---------- */

  function setCard(node, on) {
    const card = Number(node.dataset.card);
    if (on) selected.add(card);
    else selected.delete(card);
    node.classList.toggle('up', on);
    node.setAttribute('aria-pressed', String(on));
  }

  function syncPlayButton() {
    const button = el.actions.querySelector('[data-action="ddz-play"]');
    if (button) button.disabled = !selected.size || !ctx.connected();
  }

  el.hand.addEventListener('pointerdown', event => {
    const node = event.target.closest('[data-card]');
    if (!node || event.button !== 0) return;
    event.preventDefault();
    drag = { on: !selected.has(Number(node.dataset.card)), seen: new Set([node]) };
    setCard(node, drag.on);
    syncPlayButton();
  });
  document.addEventListener('pointermove', event => {
    if (!drag) return;
    const node = document.elementFromPoint(event.clientX, event.clientY)?.closest('#ddz-hand [data-card]');
    if (node && !drag.seen.has(node)) {
      drag.seen.add(node);
      setCard(node, drag.on);
      syncPlayButton();
    }
  });
  for (const type of ['pointerup', 'pointercancel']) document.addEventListener(type, () => { drag = null; });
  el.hand.addEventListener('click', event => {
    // Pointer clicks are handled on pointerdown; this is the keyboard path.
    const node = event.target.closest('[data-card]');
    if (!node || event.detail !== 0) return;
    setCard(node, !selected.has(Number(node.dataset.card)));
    syncPlayButton();
  });
  el.hand.addEventListener('contextmenu', event => {
    if (!el.hand.classList.contains('mine')) return;
    event.preventDefault();
    selected.clear();
    renderHand(table());
    syncPlayButton();
  });

  function hint() {
    const current = game();
    if (!current?.hints.length) return;
    hintIndex = (hintIndex + 1) % current.hints.length;
    selected = new Set(current.hints[hintIndex]);
    renderHand(table());
    syncPlayButton();
  }

  /* ---------- Actions ---------- */

  function showJoinError(message) {
    el.joinError.textContent = message;
    el.joinError.hidden = !message;
    el.joinCode.setAttribute('aria-invalid', String(Boolean(message)));
  }

  $('#ddz-code-form').addEventListener('submit', async event => {
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
  el.joinCode.addEventListener('input', () => showJoinError(''));
  el.search.addEventListener('input', renderLobby);

  $('#ddz-chat-form').addEventListener('submit', async event => {
    event.preventDefault();
    const draft = el.chatInput.value;
    const text = draft.trim();
    const roomId = table()?.id;
    if (!text || !roomId) return;
    if (await run('guidance', { text }) && table()?.id === roomId && el.chatInput.value === draft) el.chatInput.value = '';
  });

  $('#ddz-emotes').innerHTML = EMOTES.map(emote => `<button type="button" data-action="ddz-say" data-text="${emote}" aria-label="发送表情 ${emote}">${emote}</button>`).join('');
  $('#ddz-phrases').innerHTML = PHRASES.map(text => `<button type="button" data-action="ddz-say" data-text="${text}">${text}</button>`).join('');

  $('#ddz-sound-toggle').setAttribute('aria-pressed', String(fx.sound));
  $('#ddz-voice-toggle').setAttribute('aria-pressed', String(fx.voice));
  for (const [id, key] of [['#ddz-counter-toggle', COUNTER_KEY], ['#ddz-barrage-toggle', BARRAGE_KEY]]) {
    try {
      if (localStorage.getItem(key) === 'false') $(id).setAttribute('aria-pressed', 'false');
    } catch { /* Keep the default when storage is unavailable. */ }
  }

  function toggle(button, key) {
    const enabled = button.getAttribute('aria-pressed') !== 'true';
    button.setAttribute('aria-pressed', String(enabled));
    try { localStorage.setItem(key, String(enabled)); } catch { /* Keep it for this page only. */ }
    return enabled;
  }

  async function play() {
    const cards = [...selected];
    if (!cards.length) return;
    if (await run('ddz-play', { cards })) {
      selected.clear();
      renderHand(table());
    }
  }

  /** Handles a clicked data-action; returns false for actions it does not own. */
  function handle(action, button) {
    const room = table();
    const yes = button.dataset.yes === 'true';
    switch (action) {
      case 'ddz-quick':
      case 'ddz-practice':
        run(action);
        return true;
      case 'ddz-create':
        ctx.openDialog('ddz-create');
        return true;
      case 'ddz-filter':
        filter = button.dataset.filter;
        for (const option of document.querySelectorAll('[data-action="ddz-filter"]')) option.setAttribute('aria-pressed', String(option === button));
        renderLobby();
        return true;
      case 'ddz-lobby':
        if (room && room.seat !== -1 && room.game && room.game.phase !== 'finished') ctx.openDialog('ddz-leave');
        else if (room) run('leave');
        return true;
      case 'ddz-take-seat':
        if (room) run('join', { id: room.id });
        return true;
      case 'ddz-ready':
        if (room?.seats[room.seat]) run('ready', { ready: !room.seats[room.seat].ready });
        return true;
      case 'ddz-again':
        el.result.close();
        if (room?.seats[room.seat] && !room.seats[room.seat].ready) run('ready', { ready: true });
        return true;
      case 'ddz-close-result':
        el.result.close();
        return true;
      case 'ddz-bot':
      case 'ddz-show':
      case 'ddz-pass':
        if (action === 'ddz-pass') selected.clear();
        run(action);
        return true;
      case 'ddz-bid':
      case 'ddz-double':
      case 'ddz-trustee':
        run(action, { yes });
        return true;
      case 'ddz-play':
        play();
        return true;
      case 'ddz-hint':
        hint();
        return true;
      case 'ddz-counter':
        toggle(button, COUNTER_KEY);
        if (room) renderCounter(room);
        return true;
      case 'ddz-barrage':
        if (!toggle(button, BARRAGE_KEY)) clearBarrage();
        return true;
      case 'ddz-sound':
        fx.setSound(button.getAttribute('aria-pressed') !== 'true');
        button.setAttribute('aria-pressed', String(fx.sound));
        if (fx.sound) fx.play('bid');
        return true;
      case 'ddz-voice':
        fx.setVoice(button.getAttribute('aria-pressed') !== 'true');
        button.setAttribute('aria-pressed', String(fx.voice));
        if (fx.voice) fx.say('语音播报已开启');
        return true;
      case 'ddz-chat':
        $('#ddz-chat').scrollIntoView({ behavior: 'smooth', block: 'start' });
        el.chatInput.focus({ preventScroll: true });
        return true;
      case 'ddz-quick-talk': {
        const open = el.quick.hidden;
        el.quick.hidden = !open;
        button.setAttribute('aria-expanded', String(open));
        return true;
      }
      case 'ddz-say':
        run('guidance', { text: button.dataset.text });
        el.quick.hidden = true;
        $('#ddz-quick-toggle').setAttribute('aria-expanded', 'false');
        return true;
      default:
        return false;
    }
  }

  document.addEventListener('keydown', event => {
    if (el.view.hidden || event.target.matches('input, textarea') || event.ctrlKey || event.metaKey || event.altKey) return;
    const current = game();
    const myTurn = current?.phase === 'playing' && current.turn === mySeat();
    if (!myTurn) return;
    if (event.key === 'Enter' && selected.size && !event.target.matches('button')) {
      event.preventDefault();
      play();
    }
  });

  function leaveTable() {
    seen = null;
    selected.clear();
    handKey = '';
    lastKey = null;
    delete el.chatLog.dataset.room;
    clearBarrage();
    if (el.result.open) el.result.close();
  }

  return {
    renderLobby,
    renderTable,
    renderClock,
    handle,
    watchSide,
    refreshHubRanking: () => refreshSide('#hub-rank-ddz'),
    leaveTable,
    resultDialog: el.result
  };
}
