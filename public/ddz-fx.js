// 弈间 · 斗地主 sound: every effect is synthesized with Web Audio, and
// announcements use the browser's own speech voice. Nothing is downloaded.

const SOUND_KEY = 'yijian-ddz-sound';
const VOICE_KEY = 'yijian-ddz-voice';
const SPOKEN_RANKS = ['三', '四', '五', '六', '七', '八', '九', '十', '钩', '圈', 'K', '尖', '二', '小王', '大王'];
const KIND_WORDS = {
  tripleSingle: '三带一',
  triplePair: '三带一对',
  straight: '顺子',
  pairStraight: '连对',
  plane: '飞机',
  planeSingles: '飞机',
  planePairs: '飞机',
  fourSingles: '四带二',
  fourPairs: '四带两对',
  bomb: '炸弹',
  rocket: '王炸'
};

function stored(key) {
  try {
    return localStorage.getItem(key) !== 'false';
  } catch {
    return true;
  }
}

function store(key, value) {
  try { localStorage.setItem(key, String(value)); } catch { /* Keep it for this page only. */ }
}

const rankOf = card => (card < 52 ? card >> 2 : card - 52 + 13);

/** Words for a play, the way players call it out at the table. */
export function playWords(act) {
  if (!act) return '';
  if (!act.cards.length) return act.text === '不出' ? '不要' : act.text;
  const name = SPOKEN_RANKS[rankOf(act.cards[0])];
  if (act.kind === 'single') return name;
  if (act.kind === 'pair') return `对${name}`;
  if (act.kind === 'triple') return `三个${name}`;
  return KIND_WORDS[act.kind] ?? act.text;
}

export function createFx() {
  let context = null;
  let master = null;
  let sound = stored(SOUND_KEY);
  let voice = stored(VOICE_KEY);
  let voices = [];

  function audio() {
    if (!sound) return null;
    if (!context) {
      const Context = window.AudioContext || window.webkitAudioContext;
      if (!Context) return null;
      context = new Context();
      master = context.createGain();
      master.gain.value = 0.7;
      master.connect(context.destination);
    }
    if (context.state === 'suspended') context.resume().catch(() => {});
    return context;
  }

  // Browsers only start audio after a gesture; the first click or key unlocks it.
  const unlock = () => {
    audio();
    window.removeEventListener('pointerdown', unlock);
    window.removeEventListener('keydown', unlock);
  };
  window.addEventListener('pointerdown', unlock);
  window.addEventListener('keydown', unlock);

  if ('speechSynthesis' in window) {
    const load = () => { voices = speechSynthesis.getVoices().filter(item => /^zh/i.test(item.lang)); };
    load();
    speechSynthesis.addEventListener?.('voiceschanged', load);
  }

  function tone({ freq, to, type = 'sine', duration = 0.15, gain = 0.2, at = 0 }) {
    const ctx = audio();
    if (!ctx) return;
    const start = ctx.currentTime + at;
    const osc = ctx.createOscillator();
    const amp = ctx.createGain();
    osc.type = type;
    osc.frequency.setValueAtTime(freq, start);
    if (to) osc.frequency.exponentialRampToValueAtTime(to, start + duration);
    amp.gain.setValueAtTime(0.0001, start);
    amp.gain.exponentialRampToValueAtTime(gain, start + 0.01);
    amp.gain.exponentialRampToValueAtTime(0.0001, start + duration);
    osc.connect(amp).connect(master);
    osc.start(start);
    osc.stop(start + duration + 0.02);
  }

  function noise({ duration = 0.08, gain = 0.2, freq = 2000, filter = 'bandpass', q = 1, at = 0 }) {
    const ctx = audio();
    if (!ctx) return;
    const start = ctx.currentTime + at;
    const length = Math.ceil(ctx.sampleRate * duration);
    const buffer = ctx.createBuffer(1, length, ctx.sampleRate);
    const data = buffer.getChannelData(0);
    for (let i = 0; i < length; i++) data[i] = (Math.random() * 2 - 1) * (1 - i / length) ** 2;
    const source = ctx.createBufferSource();
    source.buffer = buffer;
    const shape = ctx.createBiquadFilter();
    shape.type = filter;
    shape.frequency.value = freq;
    shape.Q.value = q;
    const amp = ctx.createGain();
    amp.gain.value = gain;
    source.connect(shape).connect(amp).connect(master);
    source.start(start);
  }

  const notes = (freqs, { step = 0.09, ...rest }) => freqs.forEach((freq, index) => tone({ freq, at: index * step, ...rest }));

  const effects = {
    deal: () => noise({ duration: 0.045, gain: 0.18, freq: 3200, q: 0.8 }),
    play: () => {
      noise({ duration: 0.08, gain: 0.35, freq: 1500, q: 0.7 });
      tone({ freq: 180, duration: 0.07, gain: 0.08, type: 'triangle' });
    },
    pass: () => tone({ freq: 330, to: 250, duration: 0.2, gain: 0.07 }),
    bid: () => notes([523, 659], { duration: 0.2, gain: 0.1, type: 'triangle' }),
    decline: () => tone({ freq: 392, to: 330, duration: 0.18, gain: 0.06, type: 'triangle' }),
    landlord: () => notes([392, 494, 587, 784], { step: 0.07, duration: 0.5, gain: 0.09, type: 'triangle' }),
    turn: () => notes([880, 1175], { step: 0.08, duration: 0.16, gain: 0.08 }),
    tick: () => tone({ freq: 1250, duration: 0.035, gain: 0.05, type: 'square' }),
    bomb: () => {
      noise({ duration: 0.7, gain: 0.7, freq: 500, filter: 'lowpass' });
      tone({ freq: 140, to: 38, duration: 0.65, gain: 0.55 });
    },
    rocket: () => {
      tone({ freq: 220, to: 1800, duration: 0.32, gain: 0.12, type: 'sawtooth' });
      noise({ duration: 0.3, gain: 0.25, freq: 3000, filter: 'highpass', at: 0.02 });
      noise({ duration: 0.8, gain: 0.75, freq: 450, filter: 'lowpass', at: 0.32 });
      tone({ freq: 150, to: 36, duration: 0.75, gain: 0.6, at: 0.32 });
    },
    win: () => notes([523, 659, 784, 1047, 1319], { step: 0.1, duration: 0.35, gain: 0.1, type: 'triangle' }),
    lose: () => notes([392, 349, 311, 262], { step: 0.14, duration: 0.4, gain: 0.08 }),
    spring: () => notes([1047, 1319, 1568, 2093, 1568, 2093], { step: 0.06, duration: 0.25, gain: 0.06 })
  };

  function play(name) {
    if (sound) effects[name]?.();
  }

  function say(text) {
    if (!voice || !text || !('speechSynthesis' in window)) return;
    const line = new SpeechSynthesisUtterance(text);
    line.lang = 'zh-CN';
    if (voices[0]) line.voice = voices[0];
    line.rate = 1.2;
    line.volume = 0.9;
    // Stale call-outs are worse than none: a new one replaces the queue.
    speechSynthesis.cancel();
    speechSynthesis.speak(line);
  }

  return {
    play,
    say,
    get sound() { return sound; },
    get voice() { return voice; },
    setSound(on) {
      sound = on;
      store(SOUND_KEY, on);
      if (on) audio();
    },
    setVoice(on) {
      voice = on;
      store(VOICE_KEY, on);
      if (!on && 'speechSynthesis' in window) speechSynthesis.cancel();
    }
  };
}
