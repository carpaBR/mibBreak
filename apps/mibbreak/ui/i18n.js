// UI strings. Add a language by copying one block.
const STRINGS = {
  pt: {
    edit: 'editar', editing: 'editando', hd: 'HD sempre', cameras: 'câmeras', addCamera: '+ câmera',
    presets: 'modelos:', newLayout: '+ layout', rename: 'renomear', delete: 'apagar',
    editHint: 'arraste para mover · canto para redimensionar · E para sair',
    all: 'todas', layoutName: 'Nome do layout:', confirmDeleteLayout: 'Apagar o layout "{0}"?',
    confirmRemoveCam: 'Remover a câmera "{0}"? (ela sai de todos os layouts)',
    qrDrop: 'Arraste a imagem do QR da câmera aqui, escolha o arquivo ou cole o texto do QR:',
    qrPaste: '{SN:...,DT:...,SC:...}', qrNotFound: 'Nenhum QR de câmera encontrado nessa imagem.',
    name: 'nome', serial: 'número de série (SN)', user: 'usuário', password: 'senha',
    scHint: 'A senha foi preenchida com o código de segurança (SC) do QR, que é a senha de fábrica. Troque se você definiu outra.',
    mode: 'conexão', modeAuto: 'automático (rede local, senão nuvem)', modeLan: 'só rede local', modeCloud: 'só nuvem',
    host: 'IP local (opcional)', hostPh: 'ex.: 192.168.1.108',
    test: 'testar', cancel: 'cancelar', save: 'salvar', newCam: 'Nova câmera', editCam: 'Editar câmera',
    testing: 'testando… (até ~15 s)', testOk: 'OK: vídeo recebido via {0} ({1}).',
    noCams: 'Nenhuma câmera ainda.\n\nAperte A ou clique em "+ câmera".',
    emptyLayout: 'Layout vazio. Aperte E e escolha um modelo, ou adicione câmeras pela lista (C).',
    hdTile: 'HD nesta câmera (liga/desliga)',
    soundOff: 'mudo', soundOn: 'som', soundTip: 'áudio (desligado por padrão)', horizontal: 'modo horizontal', vertical: 'modo vertical',
    renameCam: 'Novo nome da câmera:', renameHint: 'duplo clique para renomear',
    play: 'assistir', pause: 'pausar', paused: 'pausada', more: 'mais opções', menu: 'menu',
    viewMosaic: 'modo mosaico (telas grandes)', viewList: 'lista',
    noCamsMobile: 'Nenhuma câmera ainda.\n\nToque em + para adicionar, ou no menu para importar um arquivo de câmeras.',
    export: 'exportar', import: 'importar', copy: 'copiar', saveFile: 'salvar arquivo', close: 'fechar',
    exportTitle: 'Exportar câmeras', importTitle: 'Importar câmeras', generate: 'gerar',
    withPasswords: 'incluir as senhas das câmeras',
    filePassword: 'senha do arquivo',
    filePasswordHint: 'Com senha do arquivo, o conteúdo é criptografado (AES-GCM) e pode ser enviado por WhatsApp/Telegram com segurança. Sem ela, as senhas das câmeras ficam legíveis no arquivo.',
    needFilePassword: 'Este arquivo está protegido: digite a senha do arquivo.',
    wrongFilePassword: 'Senha do arquivo incorreta (ou arquivo corrompido).',
    copied: 'Copiado.', saved: 'Arquivo salvo.',
    imported: 'Importadas: {0} novas, {1} atualizadas, {2} ignoradas.',
    pasteOrPick: 'Escolha o arquivo ou cole o conteúdo abaixo.',
    addToLayout: 'no layout', removeFromLayout: 'tirar', pip: 'janela flutuante', full: 'tela cheia',
    via_lan: 'rede local', via_cloud: 'nuvem', sd: 'eco', hdq: 'HD',
    st_idle: 'parada', st_connecting: 'conectando', st_live: 'ao vivo', st_retrying: 'reconectando',
    err_busy: 'Ocupada: outra sessão de nuvem está usando a câmera (app do fabricante aberto em algum celular, ou outro visualizador). Tentando de novo…',
    err_auth: 'Senha recusada pela câmera. Edite a câmera (lista C) e corrija usuário/senha.',
    err_offline: 'Câmera offline na nuvem: sem energia ou sem internet no local.',
    err_timeout: 'Sem resposta da câmera. Tentando de novo…',
    err_network: 'A conexão caiu. Reconectando…',
    err_other: 'Erro: {0}',
    noWebCodecs: 'Este sistema não tem WebCodecs, necessário para decodificar o vídeo. Atualize o WebView2 / navegador do sistema.',
    unsupportedCodec: 'Este sistema não consegue decodificar {0}. No Linux, instale o decodificador do GStreamer (pacote gst-libav ou gst-plugin-va) e reabra o app.',
    duplicate: 'Essa câmera já está cadastrada.',
    help:
`atalhos
  E        editar layout (mover / redimensionar)
  C        lista de câmeras
  A        adicionar câmera
  H        HD sempre (desliga o modo econômico)
  F        tela cheia na câmera sob o mouse
  1..9     tela cheia na câmera N do layout
  Esc      sair da tela cheia / fechar
  F11      tela cheia da janela

vídeo
  modo econômico (sub stream) por padrão;
  abrir em tela cheia (duplo clique, F ou 1..9)
  muda para o fluxo principal (HD).

nuvem
  cada câmera aceita UMA sessão de nuvem por vez.
  Enquanto este app mostra uma câmera pela nuvem,
  o app do fabricante no celular pode não abrir
  ela (e vice-versa). Ao parar de exibir, a sessão
  é liberada depois de alguns segundos.`,
  },
  en: {
    edit: 'edit', editing: 'editing', hd: 'always HD', cameras: 'cameras', addCamera: '+ camera',
    presets: 'presets:', newLayout: '+ layout', rename: 'rename', delete: 'delete',
    editHint: 'drag to move · corner to resize · E to finish',
    all: 'all', layoutName: 'Layout name:', confirmDeleteLayout: 'Delete layout "{0}"?',
    confirmRemoveCam: 'Remove camera "{0}"? (it leaves every layout)',
    qrDrop: 'Drop the camera QR image here, pick the file or paste the QR text:',
    qrPaste: '{SN:...,DT:...,SC:...}', qrNotFound: 'No camera QR found in that image.',
    name: 'name', serial: 'serial number (SN)', user: 'user', password: 'password',
    scHint: 'The password was filled with the QR safety code (SC), the factory password. Change it if you set another one.',
    mode: 'connection', modeAuto: 'automatic (LAN, else cloud)', modeLan: 'LAN only', modeCloud: 'cloud only',
    host: 'LAN IP (optional)', hostPh: 'e.g. 192.168.1.108',
    test: 'test', cancel: 'cancel', save: 'save', newCam: 'New camera', editCam: 'Edit camera',
    testing: 'testing… (up to ~15 s)', testOk: 'OK: video received via {0} ({1}).',
    noCams: 'No cameras yet.\n\nPress A or click "+ camera".',
    emptyLayout: 'Empty layout. Press E and pick a preset, or add cameras from the list (C).',
    hdTile: 'HD on this camera (toggle)',
    soundOff: 'muted', soundOn: 'sound', soundTip: 'audio (off by default)', horizontal: 'horizontal mode', vertical: 'vertical mode',
    renameCam: 'New camera name:', renameHint: 'double-click to rename',
    play: 'watch', pause: 'pause', paused: 'paused', more: 'more', menu: 'menu',
    viewMosaic: 'mosaic mode (large screens)', viewList: 'list',
    noCamsMobile: 'No cameras yet.\n\nTap + to add one, or open the menu to import a camera file.',
    export: 'export', import: 'import', copy: 'copy', saveFile: 'save file', close: 'close',
    exportTitle: 'Export cameras', importTitle: 'Import cameras', generate: 'generate',
    withPasswords: 'include camera passwords',
    filePassword: 'file password',
    filePasswordHint: 'With a file password the content is encrypted (AES-GCM) and safe to send over chat apps. Without it, camera passwords are readable in the file.',
    needFilePassword: 'This file is protected: type the file password.',
    wrongFilePassword: 'Wrong file password (or damaged file).',
    copied: 'Copied.', saved: 'File saved.',
    imported: 'Imported: {0} new, {1} updated, {2} skipped.',
    pasteOrPick: 'Pick the file or paste its content below.',
    addToLayout: 'to layout', removeFromLayout: 'remove', pip: 'floating window', full: 'fullscreen',
    via_lan: 'LAN', via_cloud: 'cloud', sd: 'eco', hdq: 'HD',
    st_idle: 'stopped', st_connecting: 'connecting', st_live: 'live', st_retrying: 'reconnecting',
    err_busy: 'Busy: another cloud session holds the camera (vendor app open on a phone, or another viewer). Retrying…',
    err_auth: 'The camera refused the password. Edit the camera (list C) and fix user/password.',
    err_offline: 'Camera offline in the cloud: no power or no internet on site.',
    err_timeout: 'No answer from the camera. Retrying…',
    err_network: 'Connection dropped. Reconnecting…',
    err_other: 'Error: {0}',
    noWebCodecs: 'This system lacks WebCodecs, needed to decode video. Update WebView2 / the system browser.',
    unsupportedCodec: 'This system cannot decode {0}. On Linux, install a GStreamer decoder (gst-libav or gst-plugin-va) and reopen the app.',
    duplicate: 'That camera is already registered.',
    help:
`shortcuts
  E        edit layout (move / resize)
  C        camera list
  A        add camera
  H        always HD (turns economy mode off)
  F        fullscreen the camera under the mouse
  1..9     fullscreen camera N of the layout
  Esc      leave fullscreen / close
  F11      window fullscreen

video
  economy mode (sub stream) by default; opening a
  camera fullscreen (double click, F or 1..9)
  switches it to the main stream (HD).

cloud
  each camera takes ONE cloud session at a time.
  While this app shows a camera through the cloud,
  the vendor phone app may fail to open it (and vice
  versa). Once nobody watches, the session is
  released after a few seconds.`,
  },
};

let LANG = 'pt';

function t(key, ...args) {
  const s = (STRINGS[LANG] && STRINGS[LANG][key]) ?? STRINGS.en[key] ?? key;
  return s.replace(/\{(\d)\}/g, (_, i) => args[i] ?? '');
}

function applyI18n(root = document) {
  root.querySelectorAll('[data-t]').forEach(el => (el.textContent = t(el.dataset.t)));
  root.querySelectorAll('[data-tp]').forEach(el => (el.placeholder = t(el.dataset.tp)));
}
