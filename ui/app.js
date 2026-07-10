'use strict';

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------
const $ = (sel) => document.querySelector(sel);
const uid = () => Math.random().toString(36).slice(2, 10);

const PROFILE_COLORS = [
  '#6d5ef2', '#0ea5e9', '#10b981', '#f59e0b',
  '#ef4444', '#ec4899', '#8b5cf6', '#14b8a6',
];
const FILTER_TYPES = {
  include: 'URL matches (regex)',
  exclude: 'URL does not match (regex)',
};
const KNOWN_HEADERS = [
  'Accept', 'Accept-Encoding', 'Accept-Language', 'Access-Control-Allow-Origin',
  'Authorization', 'Cache-Control', 'Content-Type', 'Cookie', 'Host', 'Origin',
  'Referer', 'User-Agent', 'X-Api-Key', 'X-Forwarded-For', 'X-Request-ID',
];

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'class') node.className = v;
    else if (k.startsWith('on')) node.addEventListener(k.slice(2), v);
    else if (v !== undefined && v !== null && v !== false) node.setAttribute(k, v);
  }
  for (const c of children) {
    if (c == null) continue;
    node.append(c.nodeType ? c : document.createTextNode(c));
  }
  return node;
}
function svgIcon(path) {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('viewBox', '0 0 24 24');
  const p = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  p.setAttribute('d', path);
  s.append(p);
  return s;
}
const ICONS = {
  trash: 'M9 3h6l1 2h4v2H4V5h4l1-2zm-3 6h12l-.9 12.1a1 1 0 0 1-1 .9H7.9a1 1 0 0 1-1-.9L6 9z',
  comment: 'M4 4h16a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H8l-4 4V5a1 1 0 0 1 1-1z',
  plus: 'M11 5h2v6h6v2h-6v6h-2v-6H5v-2h6V5z',
};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------
let state = null;
let info = null;
let saveTimer = null;

function newHeader() { return { enabled: true, name: '', value: '', comment: '' }; }
function newRedirect() { return { enabled: true, from: '', to: '' }; }
function newFilter() { return { enabled: true, type: 'include', value: '' }; }
function newProfile(i) {
  return {
    title: `Profile ${i + 1}`,
    color: PROFILE_COLORS[i % PROFILE_COLORS.length],
    enabled: true,
    headers: [newHeader()],
    respHeaders: [],
    redirects: [],
    filters: [],
  };
}
let selected = 0;

function profile() {
  selected = Math.min(selected, state.profiles.length - 1);
  return state.profiles[selected];
}

async function save() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    try {
      const res = await fetch('/api/state', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(state),
      });
      const { errors, active_count } = await res.json();
      showErrors(errors);
      $('#statusText').textContent = state.paused
        ? 'Paused'
        : `${active_count} active modification${active_count === 1 ? '' : 's'}`;
    } catch (e) {
      showToast('Could not reach the app — is it still running?');
    }
  }, 200);
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------
function render() {
  document.body.dataset.theme = state.theme || 'auto';
  $('#pausedBanner').hidden = !state.paused;
  $('#pauseBtn').textContent = state.paused ? 'Resume' : 'Pause';
  renderTabs();
  renderPanel();
}

function renderTabs() {
  const nav = $('#profileTabs');
  nav.replaceChildren();
  state.profiles.forEach((p, i) => {
    nav.append(el('button', {
      class: `profile-tab${i === selected ? ' selected' : ''}${p.enabled ? '' : ' disabled'}`,
      style: `--tab-color:${p.color}`,
      onclick: () => { selected = i; render(); },
    }, el('span', { class: 'dot' }), el('span', {}, p.title || 'Untitled')));
  });
  nav.append(el('button', {
    class: 'add-profile', title: 'New profile',
    onclick: () => { state.profiles.push(newProfile(state.profiles.length)); selected = state.profiles.length - 1; save(); render(); },
  }, '+'));
}

function renderPanel() {
  const p = profile();
  const panel = $('#profilePanel');
  panel.replaceChildren();

  const swatch = el('button', { class: 'color-swatch', style: `background:${p.color}`, title: 'Profile color',
    onclick: (e) => openColorPicker(e.currentTarget, p) });
  const title = el('input', { class: 'profile-title', value: p.title, spellcheck: 'false',
    oninput: (e) => { p.title = e.target.value; renderTabs(); save(); } });
  const menuBtn = el('button', { class: 'profile-menu-btn', title: 'Profile actions', onclick: (e) => openProfileMenu(e.currentTarget) }, '⋯');

  panel.append(el('div', { class: 'profile-head' }, swatch, title,
    switchEl(p.enabled, (on) => { p.enabled = on; save(); render(); }), menuBtn));

  panel.append(
    headerSection('Request headers', p.headers),
    headerSection('Response headers', p.respHeaders),
    redirectSection(p),
    filterSection(p),
  );
}

function switchEl(checked, onToggle) {
  const input = el('input', { type: 'checkbox' });
  input.checked = checked;
  input.addEventListener('change', () => onToggle(input.checked));
  return el('label', { class: 'switch' }, input, el('span', { class: 'track' }));
}

function plusIcon() {
  const s = svgIcon(ICONS.plus);
  s.setAttribute('width', '13'); s.setAttribute('height', '13'); s.style.fill = 'currentColor';
  return s;
}

function sectionHead(name, count, onAdd) {
  return el('div', { class: 'section-head' },
    el('h3', {}, name),
    count ? el('span', { class: 'count' }, String(count)) : null,
    el('span', { class: 'spacer' }),
    el('button', { class: 'add-row-btn', onclick: onAdd }, plusIcon(), 'Add'));
}

function rowEl(row, rows, cells) {
  const cb = el('input', { type: 'checkbox', title: 'Enable' });
  cb.checked = row.enabled;
  cb.addEventListener('change', () => { row.enabled = cb.checked; save(); render(); });
  const del = el('button', { class: 'row-btn del', title: 'Delete',
    onclick: () => { rows.splice(rows.indexOf(row), 1); save(); render(); showToast('Row deleted'); } }, svgIcon(ICONS.trash));
  return el('div', { class: `row${row.enabled ? '' : ' off'}` }, cb, ...cells, del);
}

function headerSection(name, rows) {
  const active = rows.filter((r) => r.enabled && r.name.trim()).length;
  const section = el('div', { class: 'section' },
    sectionHead(name, active, () => { rows.push(newHeader()); save(); render(); }));
  if (!rows.length) {
    section.append(el('div', { class: 'empty-note' }, 'None — click Add to create one.'));
    return section;
  }
  for (const row of rows) {
    const nameInput = el('input', { class: 'cell cell-name', list: 'headerNames', placeholder: 'Header name', spellcheck: 'false', value: row.name,
      oninput: (e) => { row.name = e.target.value; save(); } });
    const valInput = el('input', { class: 'cell cell-value', placeholder: 'Value (empty = remove header)', spellcheck: 'false', value: row.value,
      oninput: (e) => { row.value = e.target.value; save(); } });
    const commentBtn = el('button', { class: `row-btn${row.comment || row._c ? ' on' : ''}`, title: 'Comment',
      onclick: () => { row._c = !row._c; render(); } }, svgIcon(ICONS.comment));
    section.append(rowEl(row, rows, [nameInput, valInput, commentBtn]));
    if (row.comment || row._c) {
      section.append(el('div', { class: 'comment-row' },
        el('input', { class: 'cell', placeholder: 'Comment…', value: row.comment || '',
          oninput: (e) => { row.comment = e.target.value; save(); } })));
    }
  }
  return section;
}

function redirectSection(p) {
  const rows = p.redirects;
  const active = rows.filter((r) => r.enabled && r.from && r.to).length;
  const section = el('div', { class: 'section' },
    sectionHead('Redirect URLs', active, () => { rows.push(newRedirect()); save(); render(); }));
  if (!rows.length) {
    section.append(el('div', { class: 'empty-note' }, 'Redirect requests matching a regex to a new URL. Use \\1…\\9 for capture groups.'));
    return section;
  }
  for (const row of rows) {
    const from = el('input', { class: 'cell cell-from', placeholder: 'https://api\\.prod\\.com/(.*)', spellcheck: 'false', value: row.from,
      oninput: (e) => { row.from = e.target.value; save(); } });
    const to = el('input', { class: 'cell cell-to', placeholder: 'http://localhost:3000/\\1', spellcheck: 'false', value: row.to,
      oninput: (e) => { row.to = e.target.value; save(); } });
    section.append(rowEl(row, rows, [from, el('span', { class: 'arrow' }, '→'), to]));
  }
  return section;
}

function filterSection(p) {
  const rows = p.filters;
  const active = rows.filter((r) => r.enabled && r.value).length;
  const section = el('div', { class: 'section' },
    sectionHead('Filters', active, () => { rows.push(newFilter()); save(); render(); }));
  if (!rows.length) {
    section.append(el('div', { class: 'empty-note' }, 'No filters — this profile applies to every request.'));
    return section;
  }
  for (const row of rows) {
    const select = el('select', { class: 'cell' });
    for (const [value, label] of Object.entries(FILTER_TYPES)) {
      const opt = el('option', { value }, label);
      if (row.type === value) opt.selected = true;
      select.append(opt);
    }
    select.addEventListener('change', () => { row.type = select.value; save(); });
    const valInput = el('input', { class: 'cell cell-filter', placeholder: row.type === 'include' ? '://api\\.example\\.com/' : '\\.png$', spellcheck: 'false', value: row.value,
      oninput: (e) => { row.value = e.target.value; save(); } });
    section.append(rowEl(row, rows, [select, valInput]));
  }
  return section;
}

// ---------------------------------------------------------------------------
// Color picker
// ---------------------------------------------------------------------------
function openColorPicker(anchor, p) {
  closePops();
  const pop = el('div', { class: 'color-pop', id: 'pop' });
  for (const color of PROFILE_COLORS) {
    pop.append(el('button', { style: `background:${color}`,
      onclick: () => { p.color = color; save(); render(); closePops(); } }));
  }
  const r = anchor.getBoundingClientRect();
  pop.style.top = `${r.bottom + window.scrollY + 6}px`;
  pop.style.left = `${r.left + window.scrollX}px`;
  document.body.append(pop);
}
function closePops() { document.getElementById('pop')?.remove(); $('#menu').hidden = true; }

// ---------------------------------------------------------------------------
// Profile + import/export menu
// ---------------------------------------------------------------------------
function openProfileMenu(anchor) {
  const menu = $('#menu');
  menu.replaceChildren();
  const item = (label, fn, cls = '') => menu.append(el('button', { class: cls, onclick: () => { menu.hidden = true; fn(); } }, label));
  item('Clone profile', () => {
    const copy = JSON.parse(JSON.stringify(profile()));
    copy.title += ' (copy)';
    state.profiles.splice(selected + 1, 0, copy);
    selected += 1; save(); render();
  });
  item('Delete profile', () => {
    state.profiles.splice(selected, 1);
    if (!state.profiles.length) state.profiles.push(newProfile(0));
    selected = Math.max(0, selected - 1); save(); render(); showToast('Profile deleted');
  }, 'danger');
  showMenu(anchor);
}

function openMainMenu(anchor) {
  const menu = $('#menu');
  menu.replaceChildren();
  const item = (label, fn, cls = '') => menu.append(el('button', { class: cls, onclick: () => { menu.hidden = true; fn(); } }, label));
  item('Export this profile', () => downloadJSON([exportable(profile())], `${profile().title}.json`));
  item('Export all profiles', () => downloadJSON(state.profiles.map(exportable), 'reheader-profiles.json'));
  item('Import (ReHeader / ModHeader)', () => $('#importFile').click());
  menu.append(el('div', { class: 'sep' }));
  const themes = { auto: 'System', light: 'Light', dark: 'Dark' };
  const next = { auto: 'light', light: 'dark', dark: 'auto' };
  menu.append(el('button', { onclick: () => { state.theme = next[state.theme || 'auto']; save(); render(); menu.hidden = true; } },
    'Theme', el('span', { class: 'hint' }, themes[state.theme || 'auto'])));
  showMenu(anchor);
}

function showMenu(anchor) {
  const menu = $('#menu');
  menu.hidden = false;
  const r = anchor.getBoundingClientRect();
  const w = 240;
  menu.style.top = `${r.bottom + window.scrollY + 6}px`;
  menu.style.left = `${Math.max(8, Math.min(r.left + window.scrollX, window.innerWidth - w - 8))}px`;
}

function exportable(p) {
  return {
    title: p.title, color: p.color, enabled: p.enabled,
    headers: p.headers.map(({ enabled, name, value, comment }) => ({ enabled, name, value, comment })),
    respHeaders: p.respHeaders.map(({ enabled, name, value, comment }) => ({ enabled, name, value, comment })),
    redirects: p.redirects.map(({ enabled, from, to }) => ({ enabled, from, to })),
    filters: p.filters.map(({ enabled, type, value }) => ({ enabled, type, value })),
  };
}

function downloadJSON(data, filename) {
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
  const url = URL.createObjectURL(blob);
  const a = el('a', { href: url, download: filename.replace(/[^\w.\- ]+/g, '_') });
  document.body.append(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 4000);
}

// Accepts ReHeader's native export and ModHeader's export format.
function importProfiles(data) {
  const list = Array.isArray(data) ? data : (data.profiles || [data]);
  const out = [];
  for (const p of list) {
    if (!p || typeof p !== 'object') continue;
    const filters = (p.filters || []).map((f) => {
      const type = f.type === 'urls' ? 'include' : f.type === 'excludeUrls' ? 'exclude' : f.type;
      if (type !== 'include' && type !== 'exclude') return null;
      return { enabled: f.enabled !== false, type, value: f.value ?? f.urlRegex ?? '' };
    }).filter(Boolean);
    const redirects = (p.redirects || p.urlReplacements || []).map((r) => ({
      enabled: r.enabled !== false, from: r.from ?? r.name ?? '', to: r.to ?? r.value ?? '',
    }));
    const mapH = (h) => ({ enabled: h.enabled !== false, name: h.name || '', value: h.value || '', comment: h.comment || '' });
    out.push({
      title: p.title || p.shortTitle || 'Imported profile',
      color: p.color || p.backgroundColor || '#6d5ef2',
      enabled: true,
      headers: (p.headers || []).map(mapH),
      respHeaders: (p.respHeaders || []).map(mapH),
      redirects, filters,
    });
  }
  return out;
}

async function handleImport(file) {
  try {
    const data = JSON.parse(await file.text());
    const profiles = importProfiles(data);
    if (!profiles.length) throw new Error('no profiles found');
    state.profiles.push(...profiles);
    selected = state.profiles.length - 1;
    save(); render();
    showToast(`Imported ${profiles.length} profile${profiles.length === 1 ? '' : 's'}`);
  } catch (e) {
    showToast(`Import failed: ${e.message}`);
  }
}

// ---------------------------------------------------------------------------
// Setup card (launch, info)
// ---------------------------------------------------------------------------
async function loadInfo() {
  info = await (await fetch('/api/info')).json();
  $('#proxyPill').textContent = `proxy: 127.0.0.1:${info.proxyPort}`;
  $('#proxyPill').classList.add('ok');
  $('#proxyAddr').textContent = `127.0.0.1:${info.proxyPort}`;

  const sel = $('#browserSelect');
  sel.replaceChildren();
  if (info.browsers.length) {
    for (const b of info.browsers) sel.append(el('option', { value: b.id }, b.name));
    $('#launchBtn').disabled = false;
  } else {
    sel.append(el('option', {}, 'No Chromium browser found'));
    $('#launchBtn').disabled = true;
  }

  const flag = `--proxy-server=127.0.0.1:${info.proxyPort} --ignore-certificate-errors-spki-list=${info.spki}`;
  $('#launchCmd').textContent = flag;

  $('#upstreamInput').value = info.upstreamProxy || '';
  const hint = $('#upstreamHint');
  if (info.upstreamProxy) {
    hint.textContent = `Active: ${info.upstreamProxy}`;
  } else if (info.detectedProxy) {
    hint.textContent = `Detected: ${info.detectedProxy} (click Save to use)`;
    if (!$('#upstreamInput').value) $('#upstreamInput').value = info.detectedProxy;
  } else {
    hint.textContent = 'None detected — traffic goes direct';
  }
}

async function saveUpstream() {
  const proxy = $('#upstreamInput').value.trim();
  try {
    const res = await (await fetch('/api/upstream', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ proxy }),
    })).json();
    showToast(res.upstream
      ? `Upstream proxy set to ${res.upstream} — applied (no browser restart needed, just reload the page)`
      : 'Upstream proxy cleared — direct connection');
    setTimeout(loadInfo, 400);
  } catch (e) {
    showToast(`Could not update upstream proxy: ${e.message}`);
  }
}

async function launch() {
  const browser = $('#browserSelect').value;
  const msg = $('#launchMsg');
  msg.hidden = false; msg.classList.remove('err'); msg.textContent = 'Launching…';
  try {
    const res = await (await fetch('/api/launch', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ browser }),
    })).json();
    msg.textContent = res.message;
    msg.classList.toggle('err', !res.ok);
  } catch (e) {
    msg.textContent = `Launch failed: ${e.message}`;
    msg.classList.add('err');
  }
}

// ---------------------------------------------------------------------------
// Misc UI
// ---------------------------------------------------------------------------
function showErrors(errors) {
  const banner = $('#errorBanner');
  banner.hidden = !errors || !errors.length;
  banner.textContent = (errors || []).join('\n');
}
let toastTimer = null;
function showToast(text) {
  $('#toastText').textContent = text;
  $('#toast').hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => ($('#toast').hidden = true), 3500);
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------
async function init() {
  const dl = el('datalist', { id: 'headerNames' });
  for (const h of KNOWN_HEADERS) dl.append(el('option', { value: h }));
  document.body.append(dl);

  state = await (await fetch('/api/state')).json();
  if (!state.profiles || !state.profiles.length) state.profiles = [newProfile(0)];

  await loadInfo();
  render();
  $('#statusText').textContent = `${info.activeCount} active modification${info.activeCount === 1 ? '' : 's'}`;
  showErrors(info.errors);

  $('#launchBtn').addEventListener('click', launch);
  $('#upstreamSave').addEventListener('click', saveUpstream);
  $('#pauseBtn').addEventListener('click', () => { state.paused = !state.paused; save(); render(); });
  $('#resumeBtn').addEventListener('click', () => { state.paused = false; save(); render(); });
  $('#menuBtn').addEventListener('click', (e) => { e.stopPropagation(); if ($('#menu').hidden) openMainMenu(e.currentTarget); else $('#menu').hidden = true; });
  $('#copySpki').addEventListener('click', async () => { await navigator.clipboard.writeText(info.spki); showToast('SPKI hash copied'); });
  $('#importFile').addEventListener('change', (e) => { if (e.target.files[0]) handleImport(e.target.files[0]); e.target.value = ''; });

  document.addEventListener('click', (e) => {
    if (!$('#menu').contains(e.target) && !e.target.closest('.profile-menu-btn, #menuBtn')) $('#menu').hidden = true;
    if (!e.target.closest('.color-pop, .color-swatch')) document.getElementById('pop')?.remove();
  });
}

init();
