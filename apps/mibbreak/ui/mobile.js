// Phone layout: a vertical list of cameras, all paused until you press play.
// Each card: play/pause, HD, fullscreen, picture-in-picture (Android) and a menu.

const ICON = {
  play: 'M8 5v14l11-7z',
  pause: 'M6 5h4v14H6zm8 0h4v14h-4z',
  full: 'M7 14H5v5h5v-2H7v-3zm-2-4h2V7h3V5H5v5zm12 7h-3v2h5v-5h-2v3zM14 5v2h3v3h2V5h-5z',
  pip: 'M19 7h-8v6h8V7zm2-4H3c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h18c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 16H3V5h18v14z',
  more: 'M12 8a2 2 0 1 0 0-4 2 2 0 0 0 0 4zm0 2a2 2 0 1 0 0 4 2 2 0 0 0 0-4zm0 6a2 2 0 1 0 0 4 2 2 0 0 0 0-4z',
  plus: 'M19 13h-6v6h-2v-6H5v-2h6V5h2v6h6v2z',
  menu: 'M3 18h18v-2H3v2zm0-5h18v-2H3v2zm0-7v2h18V6H3z',
  rotate: 'M16.48 2.52c3.27 1.55 5.61 4.72 5.97 8.48h1.5C23.44 4.84 18.29 0 12 0l-.66.03 3.81 3.81 1.33-1.32zm-6.25-.77c-.59-.59-1.54-.59-2.12 0L1.75 8.11c-.59.59-.59 1.54 0 2.12l12.02 12.02c.59.59 1.54.59 2.12 0l6.36-6.36c.59-.59.59-1.54 0-2.12L10.23 1.75zm4.6 19.44L2.81 9.17l6.36-6.36 12.02 12.02-6.36 6.36zm-7.31.29C4.25 19.94 1.91 16.76 1.55 13H.05C.56 19.16 5.71 24 12 24l.66-.03-3.81-3.81-1.33 1.32z',
  soundOff: 'M16.5 12c0-1.77-1.02-3.29-2.5-4.03v2.21l2.45 2.45c.03-.2.05-.41.05-.63zm2.5 0c0 .94-.2 1.82-.54 2.64l1.51 1.51C20.63 14.91 21 13.5 21 12c0-4.28-2.99-7.86-7-8.77v2.06c2.89.86 5 3.54 5 6.71zM4.27 3 3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25c-.67.52-1.42.93-2.25 1.18v2.06c1.38-.31 2.63-.95 3.69-1.81L19.73 21 21 19.73l-9-9L4.27 3zM12 4 9.91 6.09 12 8.18V4z',
  soundOn: 'M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z',
  close: 'M19 6.41 17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12z',
};

function icon(name) {
  const NS = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(NS, 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  const path = document.createElementNS(NS, 'path');
  path.setAttribute('d', ICON[name]);
  svg.append(path);
  return svg;
}

function iconBtn(name, title, onClick, cls = 'm-btn') {
  const b = el('button', cls);
  b.type = 'button';
  b.title = title;
  b.setAttribute('aria-label', title);
  b.append(icon(name));
  b.onclick = e => {
    e.stopPropagation();
    onClick(b);
  };
  return b;
}

const androidBridge = () => window.MibBreakAndroid;
const canNativePip = () => {
  try {
    return !!androidBridge()?.pipSupported();
  } catch {
    return false;
  }
};

const mobile = {
  cards: [],
  full: null,
  pipCard: null,
  resumeAfterHide: [],
  horizontal: false,
  observer: null,
};

function setOrientation(mode) {
  try {
    androidBridge()?.setOrientation(mode);
  } catch {}
}

function mobileHdPrefs() {
  state.layouts.mobileHd ??= {};
  return state.layouts.mobileHd;
}

// ── sheet menu ─────────────────────────────────────────────────────────────
function openSheet(items) {
  const sheet = $('#m-sheet');
  const ul = sheet.querySelector('ul');
  ul.replaceChildren();
  for (const it of items) {
    const li = el('li');
    const b = el('button', it.danger ? 'danger' : '', it.label);
    b.onclick = () => {
      sheet.hidden = true;
      it.run();
    };
    li.append(b);
    ul.append(li);
  }
  const li = el('li');
  const cancel = el('button', null, t('cancel'));
  cancel.onclick = () => (sheet.hidden = true);
  li.append(cancel);
  ul.append(li);
  sheet.hidden = false;
}

// ── cards ──────────────────────────────────────────────────────────────────
function cardHd(card) {
  return state.settings.always_hd || card.hd || mobile.full === card || mobile.pipCard === card;
}

function syncCard(card) {
  const playing = !!card.player;
  card.el.classList.toggle('playing', playing);
  card.el.classList.toggle('hd', playing && cardHd(card));
  card.btnPlay.replaceChildren(icon(playing ? 'pause' : 'play'));
  card.btnPlay.title = playing ? t('pause') : t('play');
  card.btnHd.classList.toggle('on', !!card.hd);
  card.player?.setHd(cardHd(card));
  updateCardStatus(card);
}

function play(card) {
  if (card.player) return;
  card.msg.hidden = true;
  card.player = new Player(card.canvas, card.cam.id, {
    hd: cardHd(card),
    onError: text => showCardMsg(card, text, true),
  });
  card.player.start().catch(err => showCardMsg(card, String(err), true));
  syncCard(card);
}

function pause(card) {
  card.player?.stop();
  card.player = null;
  card.sound.classList.remove('on');
  card.sound.replaceChildren(icon('soundOff'));
  syncCard(card);
}

/** Sound is muted by default; the toggle only exists while playing. */
function toggleSound(card) {
  if (!card.player) return;
  const on = !card.sound.classList.contains('on');
  card.player.setMuted(!on);
  card.sound.classList.toggle('on', on);
  card.sound.replaceChildren(icon(on ? 'soundOn' : 'soundOff'));
}

// ── horizontal mode: swipe; the card that fills the screen plays ───────────
function setHorizontal(on) {
  mobile.horizontal = on;
  document.body.classList.toggle('m-horizontal', on);
  setOrientation(on ? 'landscape' : 'portrait');
  mobile.observer?.disconnect();
  mobile.observer = null;
  $('#m-rotate').title = on ? t('vertical') : t('horizontal');
  if (!on) return;
  mobile.cards.forEach(pause);
  mobile.observer = new IntersectionObserver(
    entries => {
      for (const e of entries) {
        const card = mobile.cards.find(c => c.el === e.target);
        if (!card) continue;
        if (e.intersectionRatio >= 0.6) play(card);
        else if (card.player && mobile.pipCard !== card) pause(card);
      }
    },
    { root: $('#m-list'), threshold: [0, 0.6] },
  );
  mobile.cards.forEach(c => mobile.observer.observe(c.el));
}

function showCardMsg(card, text, isErr) {
  card.msg.hidden = !text;
  card.msg.textContent = text || '';
  card.msg.classList.toggle('err', !!isErr);
}

function updateCardStatus(card) {
  const st = state.statuses[card.cam.id];
  const playing = !!card.player;
  let line;
  if (!playing) line = t('paused');
  else if (!st) line = t('st_connecting');
  else if (st.state === 'live') line = `${t('st_live')} · ${t('via_' + st.via)} · ${st.hd ? t('hdq') : t('sd')}`;
  else line = t('st_' + st.state);
  card.dot.className = 'dot ' + (playing && st ? st.state : '');
  card.sub.textContent = line;
  if (playing && st && st.state === 'retrying') showCardMsg(card, errorText(st), st.error === 'auth' || st.error === 'offline');
  else if (playing && st && st.state === 'live') showCardMsg(card, '', false);
}

function setFull(card) {
  const prev = mobile.full;
  if (prev) prev.video.classList.remove('fake-full');
  mobile.full = card;
  if (card) {
    card.video.classList.add('fake-full');
    play(card);
    history.pushState({ full: card.cam.id }, ''); // Android back leaves fullscreen
  }
  if (prev) syncCard(prev);
  if (card) syncCard(card);
}

function startPip(card) {
  if (!canNativePip()) return invoke('open_pip', { id: card.cam.id }).catch(() => {});
  play(card);
  mobile.pipCard = card;
  card.el.classList.add('pip-card');
  document.body.classList.add('in-pip');
  syncCard(card);
  const w = card.canvas.width || 16, h = card.canvas.height || 9;
  androidBridge().enterPip(w, h);
}

window.__onPip = inPip => {
  if (inPip) return;
  const card = mobile.pipCard;
  mobile.pipCard = null;
  document.body.classList.remove('in-pip');
  if (card) {
    card.el.classList.remove('pip-card');
    syncCard(card);
  }
};

function makeCard(cam) {
  const card = { cam, hd: !!mobileHdPrefs()[cam.id], player: null };
  const root = el('article', 'm-card');
  const video = el('div', 'm-video');
  const canvas = el('canvas');
  const cover = el('div', 'm-cover');
  const big = el('button', 'm-play');
  big.type = 'button';
  big.setAttribute('aria-label', t('play'));
  big.append(icon('play'));
  big.onclick = () => play(card);
  cover.append(big);
  const msg = el('div', 'm-msg');
  msg.hidden = true;
  const exit = iconBtn('close', t('close'), () => history.back(), 'm-btn m-exit');
  card.sound = iconBtn('soundOff', t('soundTip'), () => toggleSound(card), 'm-btn m-sound');
  video.append(canvas, cover, msg, card.sound, exit);
  video.addEventListener('dblclick', () => (mobile.full === card ? history.back() : setFull(card)));

  const row = el('div', 'm-row');
  card.btnPlay = iconBtn('play', t('play'), () => (card.player ? pause(card) : play(card)));
  const name = el('div', 'm-name');
  card.dot = el('span', 'dot');
  const title = el('b');
  card.nameNode = document.createTextNode(cam.name);
  title.append(card.dot, card.nameNode);
  card.sub = el('small');
  name.append(title, card.sub);
  card.btnHd = el('button', 'm-btn', 'HD');
  card.btnHd.type = 'button';
  card.btnHd.title = t('hdTile');
  card.btnHd.onclick = () => {
    card.hd = !card.hd;
    mobileHdPrefs()[cam.id] = card.hd;
    saveLayouts();
    syncCard(card);
  };
  const btnFull = iconBtn('full', t('full'), () => setFull(card));
  const btnPip = iconBtn('pip', t('pip'), () => startPip(card));
  const btnMore = iconBtn('more', t('more'), () =>
    openSheet([
      { label: t('rename'), run: () => renameCam(card.cam) },
      { label: t('edit'), run: () => openCamDialog(card.cam) },
      { label: t('delete'), danger: true, run: () => removeCam(cam) },
    ]),
  );
  row.append(card.btnPlay, name, card.btnHd, btnFull);
  if (canNativePip() || CAN_PIP) row.append(btnPip);
  row.append(btnMore);

  root.append(video, row);
  Object.assign(card, { el: root, video, canvas, msg });
  syncCard(card);
  return card;
}

function renderMobileList() {
  mobile.observer?.disconnect();
  for (const c of mobile.cards) c.player?.stop();
  mobile.cards = [];
  mobile.full = null;
  const list = $('#m-list');
  list.replaceChildren();
  $('#m-count').textContent = state.cams.length ? `${state.cams.length}` : '';
  if (!state.cams.length) {
    list.append(el('div', 'm-empty', t('noCamsMobile')));
    return;
  }
  for (const cam of state.cams) {
    const card = makeCard(cam);
    mobile.cards.push(card);
    list.append(card.el);
  }
  if (mobile.horizontal) setHorizontal(true);
}

function onMobileStatus(id) {
  for (const c of mobile.cards) if (c.cam.id === id) updateCardStatus(c);
}

function mobileMenu() {
  openSheet([
    { label: t('addCamera'), run: () => openCamDialog() },
    { label: t('import'), run: () => openTransfer('import') },
    { label: t('export'), run: () => openTransfer('export') },
    { label: t('viewMosaic'), run: () => { setHorizontal(false); setOrientation('auto'); setView('mosaic'); } },
  ]);
}

function setupMobile() {
  const root = $('#mobile');
  const bar = el('header', 'm-bar');
  const brand = el('span', 'brand', 'mibBreak');
  const count = el('span', 'count');
  count.id = 'm-count';
  const rotate = iconBtn('rotate', t('horizontal'), () => setHorizontal(!mobile.horizontal));
  rotate.id = 'm-rotate';
  bar.append(brand, count, el('span', 'spacer'), rotate, iconBtn('plus', t('addCamera'), () => openCamDialog()), iconBtn('menu', t('menu'), mobileMenu));
  const list = el('main', 'm-list');
  list.id = 'm-list';
  const sheet = el('div', 'm-sheet');
  sheet.id = 'm-sheet';
  sheet.hidden = true;
  sheet.append(el('ul'));
  sheet.onclick = e => e.target === sheet && (sheet.hidden = true);
  root.replaceChildren(bar, list, sheet);

  window.addEventListener('popstate', () => mobile.full && setFull(null));
  // Leaving the app frees the cloud sessions; coming back resumes what was playing
  document.addEventListener('visibilitychange', () => {
    if (!state.mobile || document.body.classList.contains('in-pip')) return;
    if (document.hidden) {
      mobile.resumeAfterHide = mobile.cards.filter(c => c.player);
      mobile.resumeAfterHide.forEach(pause);
    } else {
      mobile.resumeAfterHide.forEach(play);
      mobile.resumeAfterHide = [];
    }
  });
}
