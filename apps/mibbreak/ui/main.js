// Start-up, top bar, keyboard and status line.

function renderBar() {
  $('#btn-edit').textContent = state.edit ? t('editing') : t('edit');
  $('#btn-edit').classList.toggle('on', state.edit);
  $('#btn-hd').textContent = t('hd');
  $('#btn-hd').classList.toggle('on', state.settings.always_hd);
  $('#btn-cams').textContent = t('cameras');
  $('#btn-cams').classList.toggle('on', !$('#drawer').hidden);
  $('#edit-bar').hidden = !state.edit;
  document.body.classList.toggle('edit', state.edit);
}

function renderStatusLine() {
  const line = $('#status');
  line.replaceChildren();
  const sts = state.cams.map(c => state.statuses[c.id]).filter(Boolean);
  const live = sts.filter(s => s.state === 'live');
  const lan = live.filter(s => s.via === 'lan').length;
  const summary = `${state.cams.length} ${t('cameras')} · ${live.length} ${t('st_live')} (${lan} ${t('via_lan')}, ${live.length - lan} ${t('via_cloud')})`;
  line.append(el('span', live.length ? 'ok' : '', summary));
  for (const s of sts.filter(s => s.state === 'retrying')) {
    const cam = camById(s.id);
    line.append(el('span', 'err', `${cam ? cam.name : s.id}: ${errorText(s)}`));
  }
}

function toggleEdit() {
  state.edit = !state.edit;
  if (state.edit && state.full) toggleFull(state.full);
  renderBar();
}

function toggleDrawer(force) {
  $('#drawer').hidden = force === undefined ? !$('#drawer').hidden : !force;
  renderBar();
}

async function toggleAlwaysHd() {
  state.settings.always_hd = !state.settings.always_hd;
  await invoke('set_settings', { settings: state.settings });
  for (const tile of state.tiles) refreshHd(tile);
  renderBar();
}

async function newLayout() {
  const name = await ask(t('layoutName'), { input: true });
  if (!name || name === ALL) return;
  state.layouts.items[name] = { tiles: [] };
  switchLayout(name);
  if (!state.edit) toggleEdit();
}

async function renameLayout() {
  const old = state.layouts.current;
  if (old === ALL || !state.layouts.items[old]) return;
  const name = await ask(t('layoutName'), { input: true, value: old });
  if (!name || name === old || name === ALL || state.layouts.items[name]) return;
  state.layouts.items[name] = state.layouts.items[old];
  delete state.layouts.items[old];
  state.layouts.current = name;
  saveLayouts();
  renderLayoutTabs();
}

async function deleteLayout() {
  const cur = state.layouts.current;
  if (cur === ALL || !state.layouts.items[cur]) return;
  if (!(await ask(t('confirmDeleteLayout', cur)))) return;
  delete state.layouts.items[cur];
  switchLayout(ALL);
}

async function toggleWindowFullscreen() {
  const w = window.__TAURI__.window.getCurrentWindow();
  await w.setFullscreen(!(await w.isFullscreen()));
}

// ── view: phone list or mosaic ────────────────────────────────────────────
const isPhone = () => Math.min(screen.width, screen.height) < 600;

function wantMobile() {
  const v = state.settings.view || 'auto';
  return !state.pip && (v === 'list' || (v === 'auto' && isPhone()));
}

function rerender() {
  if (state.mobile) renderMobileList();
  else renderStage();
}

function applyView() {
  const m = wantMobile();
  if (m) clearStage();
  else {
    for (const c of mobile.cards) c.player?.stop();
    mobile.cards = [];
  }
  state.mobile = m;
  document.body.classList.toggle('mobile', m);
  // Phones: the app picks the orientation (button), not the rotation sensor
  if (m && !mobile.horizontal) setOrientation('portrait');
  renderBar();
  rerender();
  uiLog(`view ${m ? 'list' : 'mosaic'}: ${state.cams.length} cameras, screen ${screen.width}x${screen.height}, pip=${canNativePip()}`);
}

async function setView(view) {
  state.settings.view = view;
  await invoke('set_settings', { settings: state.settings });
  applyView();
}

function onKey(e) {
  if (e.target.closest('input, select, textarea, dialog')) return;
  if (e.ctrlKey || e.metaKey || e.altKey) return;
  const k = e.key;
  if (k === 'F11') return e.preventDefault(), toggleWindowFullscreen();
  if (state.mobile) return;
  if (state.pip) {
    if (k === 'Escape') window.__TAURI__.window.getCurrentWindow().close();
    return;
  }
  if (k === 'Escape') {
    if (state.full) toggleFull(state.full);
    else if (state.edit) toggleEdit();
    else toggleDrawer(false);
  } else if (k === 'e' || k === 'E') toggleEdit();
  else if (k === 'c' || k === 'C') toggleDrawer();
  else if (k === 'a' || k === 'A') openCamDialog();
  else if (k === 'h' || k === 'H') toggleAlwaysHd();
  else if (k === 'f' || k === 'F') {
    const tile = state.hovered || state.full;
    if (tile) toggleFull(tile);
  } else if (k === '?') $('#dlg-help').showModal();
  else if (/^[1-9]$/.test(k)) {
    const tile = state.tiles[Number(k) - 1];
    if (tile) toggleFull(tile);
  }
}

async function init() {
  state.settings = await invoke('get_settings');
  LANG = state.settings.lang || 'pt';
  document.documentElement.lang = LANG === 'pt' ? 'pt-BR' : LANG;
  applyI18n();
  $('#help-text').textContent = t('help');

  if (state.pip) document.body.classList.add('pip');

  const saved = await invoke('get_layouts');
  if (saved && saved.items) state.layouts = { current: saved.current || ALL, items: saved.items };
  for (const s of await invoke('statuses')) state.statuses[s.id] = s;
  await reloadCams();

  await listen('camera-status', ({ payload }) => {
    state.statuses[payload.id] = payload;
    for (const tile of state.tiles) if (tile.cam === payload.id) updateTileStatus(tile);
    if (state.mobile) onMobileStatus(payload.id);
    if (!$('#drawer').hidden) renderCamList();
    renderStatusLine();
  });

  $('#btn-edit').onclick = toggleEdit;
  $('#btn-hd').onclick = toggleAlwaysHd;
  $('#btn-cams').onclick = () => toggleDrawer();
  $('#btn-help').onclick = () => $('#dlg-help').showModal();
  $('#btn-add').onclick = () => openCamDialog();
  $('#btn-layout-new').onclick = newLayout;
  $('#btn-layout-rename').onclick = renameLayout;
  $('#btn-layout-delete').onclick = deleteLayout;
  document.querySelectorAll('[data-preset]').forEach(b => (b.onclick = () => applyPreset(b.dataset.preset)));
  document.addEventListener('keydown', onKey);
  wireCamDialog();
  wireTransfer();

  setupMobile();
  renderLayoutTabs();
  applyView();
  renderStatusLine();
  if (!state.cams.length && !state.pip) openCamDialog();
}

window.addEventListener('error', e => uiLog(`js error: ${e.message} @ ${e.filename}:${e.lineno}`));
window.addEventListener('unhandledrejection', e => uiLog(`js rejection: ${e.reason && (e.reason.stack || e.reason)}`));

init().catch(err => {
  uiLog(`init failed: ${err && (err.stack || err)}`);
  document.body.append(el('pre', 'empty', String(err)));
});
