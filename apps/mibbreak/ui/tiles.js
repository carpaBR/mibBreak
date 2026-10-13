// Tiles: one live camera each, placed on a 24×24 grid.

function makeTile(spec) {
  const tile = { ...spec, hoverHd: false };
  const cam = camById(spec.cam);
  const root = el('div', 'tile');
  const frame = el('div', 'frame');
  const canvas = el('canvas');
  const label = el('div', 'label');
  label.title = t('renameHint');
  label.addEventListener('dblclick', e => {
    e.stopPropagation();
    const c = camById(tile.cam);
    if (c && !state.edit) renameCam(c);
  });
  const msg = el('div', 'msg');
  msg.hidden = true;

  const tools = el('div', 'tools');
  const bPip = el('button', null, 'pip');
  bPip.title = t('pip');
  bPip.onclick = e => {
    e.stopPropagation();
    invoke('open_pip', { id: tile.cam });
  };
  const bHd = el('button', null, 'HD');
  bHd.title = t('hdTile');
  bHd.onclick = e => {
    e.stopPropagation();
    tile.hd = !tile.hd;
    refreshHd(tile);
    if (!state.pip) persistTiles();
  };
  tile.hdBtn = bHd;
  const bSound = el('button', null, t('soundOff'));
  bSound.title = t('soundTip');
  bSound.onclick = e => {
    e.stopPropagation();
    const on = !bSound.classList.contains('on');
    tile.player?.setMuted(!on);
    bSound.classList.toggle('on', on);
    bSound.textContent = on ? t('soundOn') : t('soundOff');
  };
  const bFull = el('button', null, 'full');
  bFull.title = t('full');
  bFull.onclick = e => {
    e.stopPropagation();
    toggleFull(tile);
  };
  const bOut = el('button', null, 'x');
  bOut.title = t('removeFromLayout');
  bOut.onclick = e => {
    e.stopPropagation();
    removeTile(tile);
  };
  tools.append(bSound, bHd);
  if (CAN_PIP) tools.append(bPip);
  tools.append(bFull);
  if (!state.pip) tools.append(bOut);

  const handle = el('div', 'handle');
  frame.append(canvas, label, msg, tools);
  root.append(frame, handle);
  tile.el = root;
  tile.label = label;
  tile.msg = msg;
  place(tile);

  root.addEventListener('mouseenter', () => {
    state.hovered = tile;
    clearTimeout(tile.hoverTimer);
    tile.hoverTimer = setTimeout(() => {
      tile.hoverHd = true;
      refreshHd(tile);
    }, HOVER_IN_MS);
  });
  root.addEventListener('mouseleave', () => {
    if (state.hovered === tile) state.hovered = null;
    clearTimeout(tile.hoverTimer);
    tile.hoverTimer = setTimeout(() => {
      tile.hoverHd = false;
      refreshHd(tile);
    }, HOVER_OUT_MS);
  });
  root.addEventListener('dblclick', () => !state.edit && toggleFull(tile));
  root.addEventListener('pointerdown', e => startDrag(e, tile, e.target === handle));

  tile.player = new Player(canvas, tile.cam, {
    hd: wantsHd(tile),
    onError: text => showTileMsg(tile, text, true),
  });
  tile.player.start().catch(err => showTileMsg(tile, String(err), true));
  updateTileStatus(tile, cam);
  refreshHd(tile);
  return tile;
}

function showTileMsg(tile, text, isErr) {
  tile.msg.hidden = !text;
  tile.msg.textContent = text || '';
  tile.msg.classList.toggle('err', !!isErr);
}

function errorText(st) {
  if (!st || !st.error) return '';
  return st.error === 'other' ? t('err_other', st.detail || '') : t('err_' + st.error);
}

function updateTileStatus(tile, cam = camById(tile.cam)) {
  const st = state.statuses[tile.cam];
  const name = cam ? cam.name : tile.cam;
  const parts = [name];
  if (st) {
    if (st.state === 'live') parts.push(t('via_' + st.via), st.hd ? t('hdq') : t('sd'));
    else parts.push(t('st_' + st.state));
  }
  tile.label.replaceChildren(el('span', 'dot ' + (st ? st.state : '')), document.createTextNode(parts.join(' · ')));
  if (st && st.state === 'retrying') showTileMsg(tile, errorText(st), st.error === 'auth' || st.error === 'offline');
  else if (!tile.msg.classList.contains('err') || (st && st.state === 'live')) showTileMsg(tile, '', false);
}

function clearStage() {
  for (const tile of state.tiles) {
    clearTimeout(tile.hoverTimer);
    tile.player?.stop();
  }
  state.tiles = [];
  state.full = null;
  $('#stage').replaceChildren();
}

function renderStage() {
  clearStage();
  const stage = $('#stage');
  if (!state.cams.length) {
    stage.append(el('pre', 'empty', t('noCams')));
    return;
  }
  const specs = state.pip ? [{ cam: state.pip, x: 0, y: 0, w: COLS, h: ROWS }] : layoutTiles();
  if (!specs.length) {
    stage.append(el('pre', 'empty', t('emptyLayout')));
    return;
  }
  for (const spec of specs) {
    if (!camById(spec.cam)) continue;
    const tile = makeTile(spec);
    state.tiles.push(tile);
    stage.append(tile.el);
  }
}

function toggleFull(tile) {
  const prev = state.full;
  if (prev) prev.el.classList.remove('full');
  state.full = prev === tile ? null : tile;
  if (state.full) state.full.el.classList.add('full');
  if (prev) refreshHd(prev);
  if (state.full) refreshHd(state.full);
}

function removeTile(tile) {
  ensureEditableLayout();
  tile.player?.stop();
  tile.el.remove();
  state.tiles = state.tiles.filter(x => x !== tile);
  if (state.full === tile) state.full = null;
  persistTiles();
}

/** Adds a camera to the current layout, in the first free-looking spot. */
function addTile(camId) {
  ensureEditableLayout();
  const w = 8, h = 8;
  let spot = { x: 0, y: 0 };
  search: for (let y = 0; y + h <= ROWS; y += h) {
    for (let x = 0; x + w <= COLS; x += w) {
      const free = state.tiles.every(tl => x + w <= tl.x || x >= tl.x + tl.w || y + h <= tl.y || y >= tl.y + tl.h);
      if (free) {
        spot = { x, y };
        break search;
      }
    }
  }
  const tile = makeTile({ cam: camId, ...spot, w, h });
  state.tiles.push(tile);
  $('#stage').querySelector('.empty')?.remove();
  $('#stage').append(tile.el);
  persistTiles();
}

// ── drag & resize (edit mode) ──────────────────────────────────────────────
function startDrag(e, tile, resizing) {
  if (!state.edit || e.button !== 0) return;
  if (e.target.closest('button')) return;
  e.preventDefault();
  const rect = $('#stage').getBoundingClientRect();
  const ux = rect.width / COLS, uy = rect.height / ROWS;
  const start = { px: e.clientX, py: e.clientY, x: tile.x, y: tile.y, w: tile.w, h: tile.h };
  tile.el.setPointerCapture(e.pointerId);
  tile.el.style.zIndex = 5;

  const move = ev => {
    const dx = Math.round((ev.clientX - start.px) / ux);
    const dy = Math.round((ev.clientY - start.py) / uy);
    if (resizing) {
      tile.w = Math.max(3, Math.min(COLS - tile.x, start.w + dx));
      tile.h = Math.max(3, Math.min(ROWS - tile.y, start.h + dy));
    } else {
      tile.x = Math.max(0, Math.min(COLS - tile.w, start.x + dx));
      tile.y = Math.max(0, Math.min(ROWS - tile.h, start.y + dy));
    }
    place(tile);
  };
  const up = () => {
    tile.el.removeEventListener('pointermove', move);
    tile.el.removeEventListener('pointerup', up);
    tile.el.removeEventListener('pointercancel', up);
    tile.el.style.zIndex = '';
    persistTiles();
  };
  tile.el.addEventListener('pointermove', move);
  tile.el.addEventListener('pointerup', up);
  tile.el.addEventListener('pointercancel', up);
}

function applyPreset(name) {
  ensureEditableLayout();
  const ids = state.cams.map(c => c.id);
  const specs = preset(name, ids);
  state.layouts.items[state.layouts.current].tiles = specs;
  saveLayouts();
  renderStage();
}
