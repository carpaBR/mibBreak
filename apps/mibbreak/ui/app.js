// mibbreak UI: mosaic of live cameras, named layouts, camera registry.
const { listen } = window.__TAURI__.event;
const COLS = 24, ROWS = 24;
const ALL = '__all__';
const HOVER_IN_MS = 350, HOVER_OUT_MS = 1200;
const CAN_PIP = !/Android|iPhone|iPad/i.test(navigator.userAgent); // separate windows are desktop-only

const $ = s => document.querySelector(s);
const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
};

const state = {
  cams: [],
  statuses: {},
  settings: { always_hd: false, hover_hd: true, release_after_secs: 30, lang: 'pt' },
  layouts: { current: ALL, items: {} },
  tiles: [], // { cam, x, y, w, h, el, player, hoverTimer }
  edit: false,
  full: null,
  hovered: null,
  editing: null, // camera id in the dialog, null = new
  pip: new URLSearchParams(location.search).get('pip'),
};

// ── small dialogs (window.prompt/confirm are not reliable in every webview) ──
function ask(message, { input = false, value = '' } = {}) {
  return new Promise(resolve => {
    const d = el('dialog');
    const f = el('form');
    f.method = 'dialog';
    f.append(el('p', null, message));
    let field = null;
    if (input) {
      field = el('input');
      field.value = value;
      f.append(field);
    }
    const menu = el('menu');
    const cancel = el('button', null, t('cancel'));
    cancel.value = 'cancel';
    const ok = el('button', 'primary', 'ok');
    ok.value = 'ok';
    menu.append(el('span', 'spacer'), cancel, ok);
    f.append(menu);
    d.append(f);
    document.body.append(d);
    d.addEventListener('close', () => {
      resolve(d.returnValue === 'ok' ? (input ? field.value.trim() : true) : null);
      d.remove();
    });
    d.showModal();
    (field || ok).focus();
  });
}

// ── layouts ────────────────────────────────────────────────────────────────
function gridFor(ids) {
  const n = ids.length;
  if (!n) return [];
  const cols = Math.ceil(Math.sqrt(n));
  const rows = Math.ceil(n / cols);
  return ids.map((cam, i) => ({
    cam,
    x: (i % cols) * (COLS / cols),
    y: Math.floor(i / cols) * (ROWS / rows),
    w: COLS / cols,
    h: ROWS / rows,
  }));
}

function preset(name, ids) {
  if (name === '1') return ids.slice(0, 1).map(cam => ({ cam, x: 0, y: 0, w: COLS, h: ROWS }));
  if (name === '1+5') {
    const spots = [[0, 0, 16, 16], [16, 0, 8, 8], [16, 8, 8, 8], [16, 16, 8, 8], [8, 16, 8, 8], [0, 16, 8, 8]];
    return ids.slice(0, 6).map((cam, i) => ({ cam, x: spots[i][0], y: spots[i][1], w: spots[i][2], h: spots[i][3] }));
  }
  const n = Number(name[0]);
  const ws = COLS / n, hs = ROWS / n;
  return ids.slice(0, n * n).map((cam, i) => ({ cam, x: (i % n) * ws, y: Math.floor(i / n) * hs, w: ws, h: hs }));
}

function layoutTiles() {
  const known = new Set(state.cams.map(c => c.id));
  if (state.layouts.current === ALL || !state.layouts.items[state.layouts.current]) {
    return gridFor(state.cams.map(c => c.id));
  }
  return state.layouts.items[state.layouts.current].tiles.filter(tl => known.has(tl.cam)).map(tl => ({ ...tl }));
}

let saveTimer = null;
function saveLayouts() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => invoke('set_layouts', { layouts: state.layouts }), 300);
}

/** Editing the automatic "all" view turns it into a real layout first. */
function ensureEditableLayout() {
  if (state.layouts.current !== ALL && state.layouts.items[state.layouts.current]) return;
  let i = 1;
  while (state.layouts.items[`layout ${i}`]) i++;
  const name = `layout ${i}`;
  state.layouts.items[name] = { tiles: state.tiles.map(({ cam, x, y, w, h, hd }) => ({ cam, x, y, w, h, hd: !!hd })) };
  state.layouts.current = name;
  renderLayoutTabs();
}

function persistTiles() {
  const cur = state.layouts.items[state.layouts.current];
  if (!cur) return;
  cur.tiles = state.tiles.map(({ cam, x, y, w, h, hd }) => ({ cam, x, y, w, h, hd: !!hd }));
  saveLayouts();
}

function renderLayoutTabs() {
  const nav = $('#layouts');
  nav.replaceChildren();
  const names = [ALL, ...Object.keys(state.layouts.items)];
  for (const name of names) {
    const b = el('button', name === state.layouts.current ? 'on' : '', name === ALL ? t('all') : name);
    b.onclick = () => switchLayout(name);
    nav.append(b);
  }
}

function switchLayout(name) {
  state.layouts.current = name;
  saveLayouts();
  renderLayoutTabs();
  renderStage();
}

// ── tiles ──────────────────────────────────────────────────────────────────
function camById(id) {
  return state.cams.find(c => c.id === id);
}

function place(tile) {
  Object.assign(tile.el.style, {
    left: `${(tile.x / COLS) * 100}%`,
    top: `${(tile.y / ROWS) * 100}%`,
    width: `${(tile.w / COLS) * 100}%`,
    height: `${(tile.h / ROWS) * 100}%`,
  });
}

function wantsHd(tile) {
  return state.settings.always_hd || !!tile.hd || state.full === tile || (state.settings.hover_hd && tile.hoverHd);
}

function refreshHd(tile) {
  const hd = wantsHd(tile);
  tile.player?.setHd(hd);
  tile.el.classList.toggle('hd', hd);
  tile.hdBtn?.classList.toggle('on', !!tile.hd);
}

