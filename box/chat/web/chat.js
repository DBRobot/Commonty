// Chat: the box's models, and your conversations with them. The model box
// reads a conversation while it answers and keeps nothing (llama-cpp.nix);
// the conversations themselves are sealed on this device into your library,
// like everything else there, so any device that opens the library has
// them and the box that stores them cannot read them. The demo has no
// library of its own: its chats stay in the tab.

import { me } from './shell.js';
import { unlock, fetchPlain, put, mkdir, trash, save } from './library.js';
import { marked } from '/_dd/web/marked.js';

const $ = (id) => document.getElementById(id);
const DIR = '.commonty/chats';

function el(tag, props = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') e.className = v;
    else if (k === 'text') e.textContent = v;
    else e.setAttribute(k, v);
  }
  e.append(...kids);
  return e;
}
function icon(name, cls = 'i') {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('class', cls);
  s.setAttribute('aria-hidden', 'true');
  const u = document.createElementNS('http://www.w3.org/2000/svg', 'use');
  u.setAttribute('href', `#i-${name}`);
  s.append(u);
  return s;
}
const newId = () => [...crypto.getRandomValues(new Uint8Array(9))].map((b) => b.toString(16).padStart(2, '0')).join('');
const now = () => Date.now();

// ---- where chats are kept

// in the library, sealed: a file per chat and a small index of them
class Sealed {
  constructor(lib) {
    this.lib = lib;
    this.dirs = null;
  }
  async read(name, empty) {
    try {
      return JSON.parse(new TextDecoder().decode(await fetchPlain(this.lib, `${DIR}/${name}`)));
    } catch {
      return empty;
    }
  }
  async write(name, value) {
    this.dirs ||= mkdir(this.lib, '.commonty').then(() => mkdir(this.lib, DIR));
    await this.dirs;
    await put(this.lib, `${DIR}/${name}`, new Blob([JSON.stringify(value)]));
  }
  async remove(name) {
    await trash(this.lib, `${DIR}/${name}`);
  }
}

// in this tab only, gone when it closes
class InTab {
  constructor() {
    this.m = new Map();
  }
  async read(name, empty) {
    return this.m.has(name) ? structuredClone(this.m.get(name)) : empty;
  }
  async write(name, value) {
    this.m.set(name, structuredClone(value));
  }
  async remove(name) {
    this.m.delete(name);
  }
}

// ---- state

let store = new InTab();
let kept = false;                 // whether the store is the library
let models = [];                  // [{ id, name, description }]
let awake = new Set();            // model ids loaded right now
let settings = { model: null, think: false, name: '', instructions: '' };
let index = [];                   // [{ id, title, model, updated }]
let current = null;               // the open chat
let chosen = null;                // the model the next answer comes from
let busy = null;                  // the answer being written: { ctrl }
let demo = false;
const searchOn = () => !demo && $('search-web').getAttribute('aria-pressed') === 'true';

const modelName = (id) => models.find((m) => m.id === id)?.name || id || 'a model';

// ---- models

async function loadModels() {
  const r = await fetch('/v1/models');
  if (r.status === 401) {
    location.href = '/_dd/login?rd=/';
    return;
  }
  if (!r.ok) throw new Error(`the box said ${r.status}`);
  const d = await r.json();
  models = (d.data || []).map((m) => ({ id: m.id, name: m.name || m.id, description: m.description || '' }));
  await loadAwake();
}

async function loadAwake() {
  try {
    const r = await fetch('/running');
    if (!r.ok) return;
    const d = await r.json();
    awake = new Set((d.running || []).filter((m) => !m.state || m.state === 'ready').map((m) => m.model));
  } catch {
    // the picker shows no states; nothing else depends on it
  }
  drawPicker();
}

function drawPicker() {
  if (!chosen || !models.some((m) => m.id === chosen)) chosen = settings.model && models.some((m) => m.id === settings.model) ? settings.model : models[0]?.id;
  $('model-name').textContent = modelName(chosen);
  $('ta').placeholder = models.length > 1 ? `Message ${modelName(chosen)}` : 'Ask anything';
  const menu = $('menu');
  menu.replaceChildren(...models.map((m) => {
    const up = awake.has(m.id);
    const o = el('button', { type: 'button', class: 'opt', role: 'option', 'aria-selected': String(m.id === chosen) },
      el('b', {}, m.name, el('span', { class: `state ${up ? 'ready' : ''}` }, el('i'), up ? 'ready' : 'asleep')),
      el('small', { text: m.description }),
      icon('check', 'i tick'));
    o.onclick = () => {
      chosen = m.id;
      openMenu(false);
      drawPicker();
    };
    return o;
  }));
  if (models.length > 1) {
    menu.append(el('p', { class: 'note', text: 'One model is awake on a box at a time. A sleeping one takes a moment to wake, and the other goes to sleep.' }));
  }
  const s = $('s-model');
  s.replaceChildren(...models.map((m) => el('option', { value: m.id, text: m.name })));
  s.value = settings.model && models.some((m) => m.id === settings.model) ? settings.model : models[0]?.id || '';
}

function openMenu(on) {
  $('menu').hidden = !on;
  $('model-btn').setAttribute('aria-expanded', String(on));
}

// ---- the list of chats

function drawList() {
  const q = $('search').value.trim().toLowerCase();
  const items = index.filter((c) => !q || c.title.toLowerCase().includes(q)).sort((a, b) => b.updated - a.updated);
  const day = 86400000;
  const today = new Date().setHours(0, 0, 0, 0);
  const group = (t) => (t >= today ? 'Today' : t >= today - day ? 'Yesterday' : t >= today - 7 * day ? 'Previous 7 days' : new Date(t).toLocaleDateString(undefined, { month: 'long', year: 'numeric' }));
  const out = [];
  let last = null;
  for (const c of items) {
    const g = group(c.updated);
    if (g !== last) {
      out.push(el('h3', { text: g }));
      last = g;
    }
    const open = el('button', { type: 'button', class: 'c', text: c.title, title: c.title });
    if (current?.id === c.id) open.setAttribute('aria-current', 'true');
    open.onclick = () => openChat(c.id);
    const del = el('button', { type: 'button', class: 'del', 'aria-label': `Delete ${c.title}` }, icon('x', 'i small'));
    del.onclick = () => removeChat(c.id);
    out.push(el('div', { class: 'row' }, open, del));
  }
  if (!out.length) out.push(el('p', { class: 'none', text: q ? 'No chat by that name.' : 'Your chats show up here.' }));
  $('list').replaceChildren(...out);
}

async function saveCurrent() {
  if (!current) return;
  current.updated = now();
  await store.write(`${current.id}.json`, current);
  index = index.filter((c) => c.id !== current.id);
  index.push({ id: current.id, title: current.title, model: current.model, updated: current.updated });
  await store.write('index.json', index);
  drawList();
}

async function openChat(id) {
  if (busy) stop();
  const c = await store.read(`${id}.json`, null);
  if (!c) {
    $('hint').textContent = 'That chat could not be opened.';
    return;
  }
  current = c;
  chosen = c.model && models.some((m) => m.id === c.model) ? c.model : chosen;
  drawPicker();
  drawChat();
  drawList();
  $('chat').classList.remove('side-open');
  $('ta').focus();
}

async function removeChat(id) {
  const c = index.find((x) => x.id === id);
  if (!confirm(`Delete “${c?.title || 'this chat'}”?${kept ? ' It goes to the trash in Files.' : ''}`)) return;
  await store.remove(`${id}.json`);
  index = index.filter((x) => x.id !== id);
  await store.write('index.json', index);
  if (current?.id === id) newChat();
  drawList();
}

function newChat() {
  if (busy) stop();
  current = null;
  chosen = settings.model && models.some((m) => m.id === settings.model) ? settings.model : chosen;
  $('think').setAttribute('aria-pressed', String(!!settings.think));
  drawPicker();
  drawChat();
  drawList();
  $('ta').focus();
}

// ---- drawing a conversation

// markdown in, html out, and nothing in it that can run or reach out: no
// scripts, no styles, and no images, which a model could be talked into
// pointing at somewhere that would learn from the request what was said
function md(text) {
  const div = el('div', { class: 'md' });
  div.innerHTML = window.DOMPurify.sanitize(marked.parse(text || '', { gfm: true }), {
    FORBID_TAGS: ['img', 'style', 'form', 'input', 'button', 'iframe', 'video', 'audio', 'source', 'picture'],
    FORBID_ATTR: ['style'],
  });
  for (const a of div.querySelectorAll('a')) {
    a.target = '_blank';
    a.rel = 'noopener noreferrer';
  }
  for (const pre of div.querySelectorAll('pre')) {
    const code = pre.querySelector('code');
    const lang = [...(code?.classList || [])].find((c) => c.startsWith('language-'))?.slice(9) || 'text';
    const copy = el('button', { type: 'button' }, icon('copy', 'i small'), 'Copy');
    copy.onclick = () => copyText(code?.textContent || pre.textContent, copy);
    const box = el('div', { class: 'codeblock' }, el('header', {}, el('span', { text: lang }), copy));
    pre.replaceWith(box);
    box.append(pre);
  }
  return div;
}

async function copyText(text, btn) {
  try {
    await navigator.clipboard.writeText(text);
    if (btn) {
      const was = btn.lastChild.textContent;
      btn.lastChild.textContent = 'Copied';
      setTimeout(() => { btn.lastChild.textContent = was; }, 1500);
    }
  } catch {
    // no clipboard here; the text is on the page to select
  }
}

function botView(m, last) {
  const box = el('div', { class: 'bot' });
  if (m.searching) {
    box.append(el('div', { class: 'waking' }, el('span', { class: 'dot' }), 'Searching the web…'));
  } else if (m.waking && !m.content && !m.think) {
    box.append(el('div', { class: 'waking' }, el('span', { class: 'dot' }), `Waking ${modelName(m.model)}. The first answer takes longer.`));
  }
  if (m.think) {
    const d = el('details', { class: 'think' },
      el('summary', {}, m.thinkSecs ? `Thought for ${m.thinkSecs} second${m.thinkSecs === 1 ? '' : 's'}` : 'Thinking', icon('down', 'i small')),
      el('div', { class: 'md', text: m.think }));
    if (!m.content && m.writing) d.open = true;
    box.append(d);
  }
  if (m.content) box.append(md(m.content));
  if (m.writing && !m.content && !m.waking && !m.think && !m.searching) box.append(el('span', { class: 'dot', 'aria-label': 'Writing' }));
  if (m.searchNote) box.append(el('p', { class: 'note', text: m.searchNote }));
  // what the answer was given to read, numbered as it cites them
  if (m.sources?.length && !m.writing) {
    box.append(el('details', { class: 'sources' },
      el('summary', {}, `${m.sources.length} source${m.sources.length === 1 ? '' : 's'}`, icon('down', 'i small')),
      el('ol', {}, ...m.sources.map((r) => el('li', {},
        el('a', { href: r.url, target: '_blank', rel: 'noopener noreferrer nofollow', text: r.title || r.url }),
        el('span', { text: hostOf(r.url) }))))));
  }
  if (m.error) box.append(el('p', { class: 'err', text: m.error }));
  if (!m.writing) {
    const acts = el('div', { class: 'acts' });
    const copy = el('button', { type: 'button', title: 'Copy', 'aria-label': 'Copy' }, icon('copy'));
    copy.onclick = () => copyText(m.content);
    acts.append(copy);
    if (last) {
      const again = el('button', { type: 'button', title: 'Try again', 'aria-label': 'Try again' }, icon('retry'));
      again.onclick = () => retry(chosen);
      acts.append(again);
    }
    const bits = [modelName(m.model)];
    if (m.stats?.tps) bits.push(`${m.stats.tps.toFixed(1)} tok/s`);
    if (m.stopped) bits.push('stopped');
    acts.append(el('span', { class: 'meta', text: bits.join(' · ') }));
    box.append(acts);
  }
  return box;
}

function drawChat() {
  const log = $('log');
  const msgs = current?.messages || [];
  $('chat').classList.toggle('fresh', !msgs.length);
  const lastBot = msgs.map((m) => m.role).lastIndexOf('assistant');
  log.replaceChildren(...msgs.map((m, i) => (m.role === 'user' ? el('div', { class: 'you', text: m.content }) : botView(m, i === lastBot))));
  document.title = current?.title ? `${current.title} · Chat` : 'Chat';
}

let frame = 0;
function redrawSoon(scroll) {
  if (frame) return;
  frame = requestAnimationFrame(() => {
    frame = 0;
    const s = $('scroll');
    const atBottom = s.scrollHeight - s.scrollTop - s.clientHeight < 80;
    drawChat();
    if (scroll || atBottom) s.scrollTop = s.scrollHeight;
  });
}

// ---- asking

function titleFrom(text) {
  const line = text.trim().split('\n')[0];
  return line.length > 60 ? `${line.slice(0, 57).trimEnd()}…` : line || 'New chat';
}

function hostOf(url) {
  try {
    return new URL(url).host;
  } catch {
    return '';
  }
}

// What the web says about the question, for the model to read beside it.
// The model itself has no network: the gate asks SearXNG on the box, and
// titles, links and a line of each come back.
async function webResults(q, m, signal) {
  m.searching = true;
  redrawSoon(true);
  try {
    const r = await fetch(`/search?q=${encodeURIComponent(q.slice(0, 300))}`, { signal });
    if (!r.ok) throw new Error(r.status === 404 ? 'web search is not on this box' : `the search did not answer (${r.status})`);
    const found = (await r.json()).results || [];
    if (!found.length) {
      m.searchNote = 'The web search found nothing; this answer is from the model alone.';
      return [];
    }
    m.sources = found;
    const list = found.map((x, i) => `[${i + 1}] ${x.title}\n${x.url}\n${x.content}`).join('\n\n');
    return [{
      role: 'system',
      content: `Web search results for the user's latest message, fetched just now:\n\n${list}\n\nUse them where they help and cite them as [1], [2] and so on. If they do not answer the question, say so rather than guessing.`,
    }];
  } catch (e) {
    if (e.name === 'AbortError') throw e;
    m.searchNote = `No web search this time: ${e.message}. This answer is from the model alone.`;
    return [];
  } finally {
    m.searching = false;
  }
}

function system() {
  const bits = [];
  if (settings.name.trim()) bits.push(`The user's name is ${settings.name.trim()}.`);
  if (settings.instructions.trim()) bits.push(settings.instructions.trim());
  return bits.length ? [{ role: 'system', content: bits.join('\n\n') }] : [];
}

async function send(text) {
  if (busy || !text.trim() || !chosen) return;
  if (!current) current = { id: newId(), title: titleFrom(text), model: chosen, messages: [], created: now(), updated: now() };
  current.model = chosen;
  current.messages.push({ role: 'user', content: text });
  $('ta').value = '';
  grow();
  await answer();
}

async function retry(model) {
  if (busy || !current) return;
  const i = current.messages.map((m) => m.role).lastIndexOf('assistant');
  if (i >= 0) current.messages.splice(i, 1);
  chosen = model || chosen;
  drawPicker();
  await answer();
}

function stop() {
  busy?.ctrl.abort();
}

async function answer() {
  const think = $('think').getAttribute('aria-pressed') === 'true';
  const m = { role: 'assistant', content: '', think: '', model: chosen, writing: true, waking: !awake.has(chosen) };
  current.messages.push(m);
  const ctrl = new AbortController();
  busy = { ctrl };
  setBusy(true);
  redrawSoon(true);
  const history = current.messages.slice(0, -1).map(({ role, content }) => ({ role, content }));
  let thinkStart = 0;
  try {
    // the results go just before the question, so everything earlier in the
    // chat is the same as last time and the model reuses what it read
    if (searchOn()) {
      const q = history[history.length - 1]?.content || '';
      history.splice(history.length - 1, 0, ...(await webResults(q, m, ctrl.signal)));
    }
    const r = await fetch('/v1/chat/completions', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        model: chosen,
        stream: true,
        messages: [...system(), ...history],
        chat_template_kwargs: { enable_thinking: think },
      }),
      signal: ctrl.signal,
    });
    if (r.status === 401) {
      location.href = '/_dd/login?rd=/';
      return;
    }
    if (r.status === 403 && demo) throw new Error('The demo gets ten questions an hour, and this hour’s are used. Try again later.');
    if (!r.ok) throw new Error(`The box could not answer (${r.status}). ${(await r.text()).slice(0, 200)}`);
    const reader = r.body.getReader();
    const dec = new TextDecoder();
    let buf = '';
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buf += dec.decode(value, { stream: true });
      let nl;
      while ((nl = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, nl).trim();
        buf = buf.slice(nl + 1);
        if (!line.startsWith('data:')) continue;
        const data = line.slice(5).trim();
        if (data === '[DONE]') continue;
        let j;
        try {
          j = JSON.parse(data);
        } catch {
          continue;
        }
        if (j.error) throw new Error(j.error.message || 'the model stopped');
        const d = j.choices?.[0]?.delta || {};
        if (d.reasoning_content) {
          if (!thinkStart) thinkStart = now();
          m.think += d.reasoning_content;
          m.waking = false;
        }
        if (d.content) {
          if (thinkStart && !m.thinkSecs) m.thinkSecs = Math.max(1, Math.round((now() - thinkStart) / 1000));
          m.content += d.content;
          m.waking = false;
        }
        if (j.timings?.predicted_per_second) m.stats = { tps: j.timings.predicted_per_second, tokens: j.timings.predicted_n };
        redrawSoon();
      }
    }
    awake.add(chosen);
  } catch (e) {
    if (e.name === 'AbortError') m.stopped = true;
    else m.error = e.message;
  } finally {
    if (thinkStart && !m.thinkSecs) m.thinkSecs = Math.max(1, Math.round((now() - thinkStart) / 1000));
    m.writing = false;
    m.waking = false;
    busy = null;
    setBusy(false);
    redrawSoon();
    if (!m.content && !m.think && !m.stopped && !m.error) m.error = 'No answer came back.';
    try {
      await saveCurrent();
    } catch (e) {
      $('hint').textContent = `Not saved: ${e.message}`;
      $('hint').classList.add('bad');
    }
    loadAwake();
  }
}

function setBusy(on) {
  const b = $('send');
  b.replaceChildren(icon(on ? 'stop' : 'up'));
  b.setAttribute('aria-label', on ? 'Stop' : 'Send');
  b.disabled = !on && !$('ta').value.trim();
}

function grow() {
  const t = $('ta');
  t.style.height = 'auto';
  t.style.height = `${Math.min(t.scrollHeight, 220)}px`;
  if (!busy) $('send').disabled = !t.value.trim();
}

// ---- settings

let settingsTimer = 0;
function keepSettings() {
  clearTimeout(settingsTimer);
  settingsTimer = setTimeout(() => store.write('settings.json', settings).catch(() => {}), 500);
}

function openSettings(on) {
  $('settings').hidden = !on;
  $('veil').hidden = !on;
  if (on) {
    $('s-think').setAttribute('aria-checked', String(!!settings.think));
    $('s-name').value = settings.name;
    $('s-inst').value = settings.instructions;
    $('settings').querySelector('nav button[aria-current="true"]').focus();
  } else {
    $('open-settings').focus();
  }
}

async function exportAll() {
  const all = [];
  for (const c of index) {
    const full = await store.read(`${c.id}.json`, null);
    if (full) all.push({ title: full.title, model: full.model, created: full.created, messages: full.messages.map(({ role, content, think, model }) => ({ role, content, ...(think ? { think } : {}), ...(model ? { model } : {}) })) });
  }
  save(new TextEncoder().encode(JSON.stringify({ exported: new Date().toISOString(), chats: all }, null, 2)), 'chats.json');
}

async function wipe() {
  if (!confirm(`Delete all ${index.length} chats?${kept ? ' They go to the trash in Files.' : ''}`)) return;
  for (const c of index) await store.remove(`${c.id}.json`);
  index = [];
  await store.write('index.json', index);
  newChat();
  openSettings(false);
}

// ---- start

function wire() {
  $('composer').onsubmit = (e) => {
    e.preventDefault();
    if (busy) stop();
    else send($('ta').value);
  };
  $('ta').addEventListener('input', grow);
  $('ta').addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      if (!busy) send($('ta').value);
    }
  });
  addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    if (!$('settings').hidden) openSettings(false);
    else if (!$('menu').hidden) openMenu(false);
    else if (busy) stop();
  });
  $('think').onclick = () => $('think').setAttribute('aria-pressed', String($('think').getAttribute('aria-pressed') !== 'true'));
  $('search-web').onclick = () => $('search-web').setAttribute('aria-pressed', String(!searchOn()));
  $('model-btn').onclick = (e) => {
    e.stopPropagation();
    openMenu($('menu').hidden);
  };
  document.addEventListener('click', (e) => {
    if (!$('menu').hidden && !$('menu').contains(e.target)) openMenu(false);
  });
  $('new').onclick = newChat;
  $('search').addEventListener('input', drawList);
  $('toggle-side').onclick = () => $('chat').classList.toggle('side-open');
  $('open-settings').onclick = () => openSettings(true);
  $('close-settings').onclick = () => openSettings(false);
  $('veil').onclick = () => openSettings(false);
  for (const b of document.querySelectorAll('.settings nav button')) {
    b.onclick = () => {
      for (const x of document.querySelectorAll('.settings nav button')) x.setAttribute('aria-current', String(x === b));
      for (const p of document.querySelectorAll('.settings .pane')) p.hidden = p.dataset.pane !== b.dataset.pane;
    };
  }
  $('s-model').onchange = () => {
    settings.model = $('s-model').value;
    keepSettings();
  };
  $('s-think').onclick = () => {
    settings.think = $('s-think').getAttribute('aria-checked') !== 'true';
    $('s-think').setAttribute('aria-checked', String(settings.think));
    keepSettings();
  };
  $('s-name').addEventListener('input', () => {
    settings.name = $('s-name').value;
    keepSettings();
  });
  $('s-inst').addEventListener('input', () => {
    settings.instructions = $('s-inst').value;
    keepSettings();
  });
  $('export').onclick = () => exportAll().catch((e) => alert(e.message));
  $('wipe').onclick = () => wipe().catch((e) => alert(e.message));
  // the picker's states follow the box while the page is looked at
  setInterval(() => {
    if (!document.hidden && !busy) loadAwake();
  }, 15000);
}

function sayKept(text, warn) {
  const k = $('kept');
  k.querySelector('span').textContent = text;
  k.classList.toggle('warn', !!warn);
}

async function start() {
  wire();
  newChat();
  const who = await me();
  demo = !!who.demo;
  // members only: the demo would make the box a search proxy for anyone
  $('search-web').hidden = demo;
  if (settings.name === '' && !demo) $('hello').textContent = 'What can I help with?';
  const models$ = loadModels().catch((e) => {
    $('hint').textContent = `The models could not be listed: ${e.message}`;
    $('hint').classList.add('bad');
  });
  if (demo) {
    sayKept('This is the demo: chats stay in this tab and go when it closes.');
  } else {
    sayKept('Opening your library…');
    try {
      const r = await unlock(who.user);
      if (r.ok) {
        const pending = current;
        store = new Sealed(r.ok);
        kept = true;
        settings = { ...settings, ...(await store.read('settings.json', {})) };
        index = await store.read('index.json', []);
        sayKept('Your chats are sealed in your library. The model box sees one only while it answers.');
        // a chat begun while the library was opening is kept now too
        if (pending?.messages.length) await saveCurrent();
      } else {
        sayKept('Chats are not being kept: this device cannot open your library yet. Open Files to set it up.', true);
      }
    } catch (e) {
      sayKept(`Chats are not being kept here: ${e.message}`, true);
    }
  }
  await models$;
  if (!current?.messages.length) {
    $('think').setAttribute('aria-pressed', String(!!settings.think));
    chosen = settings.model && models.some((m) => m.id === settings.model) ? settings.model : chosen;
  }
  if (settings.name.trim()) $('hello').textContent = `What can I help with, ${settings.name.trim()}?`;
  drawPicker();
  drawList();
}

start();
