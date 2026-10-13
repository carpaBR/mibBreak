// Camera list (drawer) and the add / edit dialog.

async function reloadCams() {
  state.cams = await invoke('list_cameras');
  renderCamList();
}

function renderCamList() {
  const list = $('#cam-list');
  list.replaceChildren();
  for (const cam of state.cams) {
    const st = state.statuses[cam.id];
    const li = el('li');
    const top = el('div', 'top');
    top.append(el('span', 'dot ' + (st ? st.state : '')), el('span', 'name', cam.name));
    const sub = el('div', 'sub', [cam.serial, cam.model, st && st.state === 'live' ? t('via_' + st.via) : st ? t('st_' + st.state) : '']
      .filter(Boolean)
      .join(' · '));
    const err = st && st.state === 'retrying' ? el('div', 'sub', errorText(st)) : null;
    const acts = el('div', 'acts');
    const bAdd = el('button', null, '+ ' + t('addToLayout'));
    bAdd.onclick = () => addTile(cam.id);
    const bPip = el('button', null, 'pip');
    bPip.onclick = () => invoke('open_pip', { id: cam.id });
    const bRename = el('button', null, t('rename'));
    bRename.onclick = () => renameCam(cam);
    const bEdit = el('button', null, t('edit'));
    bEdit.onclick = () => openCamDialog(cam);
    const bDel = el('button', null, t('delete'));
    bDel.onclick = () => removeCam(cam);
    acts.append(bAdd);
    if (CAN_PIP) acts.append(bPip);
    acts.append(bRename, bEdit, bDel);
    li.append(top, sub);
    if (err) li.append(err);
    li.append(acts);
    list.append(li);
  }
}

/** Renames a camera everywhere, without interrupting its video. */
async function renameCam(cam) {
  const name = await ask(t('renameCam'), { input: true, value: cam.name });
  if (!name || name === cam.name) return;
  await invoke('update_camera', { id: cam.id, patch: { name } });
  await reloadCams();
  for (const tile of state.tiles) if (tile.cam === cam.id) updateTileStatus(tile);
  for (const card of mobile.cards) {
    if (card.cam.id !== cam.id) continue;
    card.cam = camById(cam.id) || card.cam;
    card.nameNode.textContent = name;
  }
}

async function removeCam(cam) {
  if (!(await ask(t('confirmRemoveCam', cam.name)))) return;
  await invoke('remove_camera', { id: cam.id });
  for (const l of Object.values(state.layouts.items)) l.tiles = l.tiles.filter(tl => tl.cam !== cam.id);
  saveLayouts();
  await reloadCams();
  rerender();
}

// ── dialog ─────────────────────────────────────────────────────────────────
const form = () => $('#cam-form');

function fillFromQr(info) {
  if (!info) return false;
  const f = form();
  f.serial.value = info.serial;
  f.model.value = info.model || '';
  if (!f.name.value) f.name.value = info.model ? `${info.model} ${info.serial.slice(-4)}` : info.serial;
  if (info.safety_code && !f.password.value) {
    f.password.value = info.safety_code;
    $('#sc-hint').hidden = false;
  }
  return true;
}

function setTestResult(text, kind) {
  const p = $('#test-result');
  p.textContent = text;
  p.className = 'hint ' + (kind || '');
}

function openCamDialog(cam = null) {
  state.editing = cam ? cam.id : null;
  const f = form();
  f.reset();
  $('#cam-form-title').textContent = cam ? t('editCam') : t('newCam');
  $('#qr-zone').hidden = !!cam;
  $('#sc-hint').hidden = true;
  setTestResult('', '');
  f.serial.readOnly = !!cam;
  if (cam) {
    f.name.value = cam.name;
    f.serial.value = cam.serial;
    f.user.value = cam.user;
    f.mode.value = cam.mode;
    f.host.value = cam.host || '';
    f.model.value = cam.model || '';
    f.password.placeholder = '••••••';
  } else {
    f.password.placeholder = '';
  }
  $('#dlg-cam').showModal();
  (cam ? f.name : f.serial).focus();
}

async function readQrFile(file) {
  const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
  const info = await invoke('decode_qr_image', { bytes }).catch(() => null);
  if (!fillFromQr(info)) setTestResult(t('qrNotFound'), 'err');
}

function wireCamDialog() {
  const f = form();
  const zone = $('#qr-zone');
  $('#qr-file').addEventListener('change', e => e.target.files[0] && readQrFile(e.target.files[0]));
  $('#qr-text').addEventListener('input', async e => {
    const info = await invoke('parse_qr_text', { text: e.target.value });
    if (info) fillFromQr(info);
  });
  zone.addEventListener('dragover', e => {
    e.preventDefault();
    zone.classList.add('drag');
  });
  zone.addEventListener('dragleave', () => zone.classList.remove('drag'));
  zone.addEventListener('drop', e => {
    e.preventDefault();
    zone.classList.remove('drag');
    const file = e.dataTransfer.files[0];
    if (file) readQrFile(file);
  });
  f.password.addEventListener('input', () => ($('#sc-hint').hidden = true));

  $('#btn-test').addEventListener('click', async () => {
    if (!f.serial.value.trim()) return f.serial.focus();
    setTestResult(t('testing'), '');
    $('#btn-test').disabled = true;
    const r = await invoke('test_camera', {
      serial: f.serial.value,
      user: f.user.value,
      password: f.password.value,
      host: f.mode.value === 'cloud' ? null : f.host.value,
    }).finally(() => ($('#btn-test').disabled = false));
    if (r.ok) setTestResult(t('testOk', t('via_' + r.via), r.codec), 'ok');
    else setTestResult(r.error === 'other' ? t('err_other', r.detail || '') : t('err_' + r.error), 'err');
  });

  $('#dlg-cam').addEventListener('close', async () => {
    if ($('#dlg-cam').returnValue !== 'save') return;
    const data = {
      name: f.name.value,
      user: f.user.value,
      password: f.password.value,
      mode: f.mode.value,
      host: f.host.value,
    };
    try {
      if (state.editing) {
        await invoke('update_camera', { id: state.editing, patch: data });
      } else {
        await invoke('add_camera', { cam: { ...data, serial: f.serial.value, model: f.model.value || null } });
      }
    } catch (err) {
      await ask(err === 'duplicate' ? t('duplicate') : String(err));
      return;
    }
    await reloadCams();
    rerender();
  });
}
