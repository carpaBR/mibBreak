// Import / export of the camera list, optionally encrypted with a file password
// (PBKDF2-SHA256 → AES-256-GCM, WebCrypto) so it can travel over chat apps.

const ENC_FORMAT = 'mibbreak-cameras-encrypted';
const ENC_FORMATS = [ENC_FORMAT, 'mibobreak-cameras-encrypted']; // + files from builds before the rename
const PBKDF2_ITERATIONS = 310000;

const b64 = {
  enc: buf => btoa(String.fromCharCode(...new Uint8Array(buf))),
  dec: str => Uint8Array.from(atob(str), c => c.charCodeAt(0)),
};

async function fileKey(password, salt, iterations) {
  const base = await crypto.subtle.importKey('raw', new TextEncoder().encode(password), 'PBKDF2', false, ['deriveKey']);
  return crypto.subtle.deriveKey(
    { name: 'PBKDF2', hash: 'SHA-256', salt, iterations },
    base,
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt'],
  );
}

async function encryptText(text, password) {
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const key = await fileKey(password, salt, PBKDF2_ITERATIONS);
  const data = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, new TextEncoder().encode(text));
  return JSON.stringify(
    { format: ENC_FORMAT, version: 1, kdf: 'PBKDF2-SHA256', iterations: PBKDF2_ITERATIONS, salt: b64.enc(salt), iv: b64.enc(iv), data: b64.enc(data) },
    null,
    2,
  );
}

async function decryptText(wrapper, password) {
  const key = await fileKey(password, b64.dec(wrapper.salt), wrapper.iterations || PBKDF2_ITERATIONS);
  const plain = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: b64.dec(wrapper.iv) }, key, b64.dec(wrapper.data));
  return new TextDecoder().decode(plain);
}

let ioMode = 'export';

function ioResult(text, kind) {
  const p = $('#io-result');
  p.textContent = text;
  p.className = 'hint ' + (kind || '');
}

function openTransfer(mode) {
  ioMode = mode;
  $('#io-title').textContent = mode === 'export' ? t('exportTitle') : t('importTitle');
  $('#io-export').hidden = mode !== 'export';
  $('#io-import').hidden = mode !== 'import';
  $('#io-pass-in-wrap').hidden = true;
  $('#io-go').textContent = mode === 'export' ? t('generate') : t('import');
  $('#io-copy').hidden = mode !== 'export';
  $('#io-save').hidden = mode !== 'export' || !CAN_PIP; // saving files is desktop-only
  $('#io-text').value = '';
  $('#io-text').readOnly = mode === 'export';
  $('#io-file').value = '';
  $('#io-pass-in').value = '';
  ioResult(mode === 'import' ? t('pasteOrPick') : '', '');
  $('#dlg-io').showModal();
}

async function runExport() {
  let text = await invoke('export_cameras', { includePasswords: $('#io-with-pass').checked });
  const pass = $('#io-pass-out').value;
  if (pass) text = await encryptText(text, pass);
  $('#io-text').value = text;
  ioResult('', '');
  return text;
}

async function runImport() {
  let text = $('#io-text').value.trim();
  if (!text) return ioResult(t('pasteOrPick'), 'err');
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (e) {
    return ioResult(String(e), 'err');
  }
  if (ENC_FORMATS.includes(parsed.format)) {
    $('#io-pass-in-wrap').hidden = false;
    const pass = $('#io-pass-in').value;
    if (!pass) {
      $('#io-pass-in').focus();
      return ioResult(t('needFilePassword'), 'err');
    }
    try {
      text = await decryptText(parsed, pass);
    } catch {
      return ioResult(t('wrongFilePassword'), 'err');
    }
  }
  try {
    const r = await invoke('import_cameras', { json: text });
    ioResult(t('imported', r.added, r.updated, r.skipped), 'ok');
    await reloadCams();
    rerender();
  } catch (err) {
    ioResult(String(err), 'err');
  }
}

function wireTransfer() {
  $('#btn-export').onclick = () => openTransfer('export');
  $('#btn-import').onclick = () => openTransfer('import');
  $('#io-go').onclick = () => (ioMode === 'export' ? runExport() : runImport());
  $('#io-file').addEventListener('change', async e => {
    const f = e.target.files[0];
    if (!f) return;
    $('#io-text').value = await f.text();
    try {
      if (ENC_FORMATS.includes(JSON.parse($('#io-text').value).format)) {
        $('#io-pass-in-wrap').hidden = false;
        $('#io-pass-in').focus();
        return ioResult(t('needFilePassword'), '');
      }
    } catch {}
    runImport();
  });
  $('#io-copy').onclick = async () => {
    const text = $('#io-text').value || (await runExport());
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      $('#io-text').select();
      document.execCommand('copy');
    }
    ioResult(t('copied'), 'ok');
  };
  $('#io-save').onclick = async () => {
    const text = $('#io-text').value || (await runExport());
    try {
      if (await invoke('save_text_file', { name: 'cameras.mibbreak.json', text })) ioResult(t('saved'), 'ok');
    } catch (err) {
      ioResult(String(err), 'err');
    }
  };
}
