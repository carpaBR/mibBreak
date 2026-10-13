// Live video in a <canvas>: subscribes to a camera in the backend and decodes the
// Annex B access units it sends with WebCodecs (hardware decoding when available).
//
// Backend messages (binary):
//   [1][json]                          stream config: {codec, hd, via}
//   [2][key u8][rtp ts u32 BE][annexb] one access unit
//   [3][json]                          audio config: {codec, sampleRate, channels, description}
//   [4][rtp ts u32 BE][frame]          one audio frame (only while unmuted)

const { invoke, Channel } = window.__TAURI__.core;

const wait = ms => new Promise(r => setTimeout(r, ms));
const uiLog = msg => invoke('ui_log', { msg }).catch(() => {});
const MAX_QUEUE = 12; // decoder backlog before we skip to the next keyframe
const AUDIO_LEAD = 0.08; // s of buffering ahead of the audio clock
const AUDIO_MAX_LAG = 0.6; // s: further behind than this, drop and catch up

// G.711 → linear PCM
function g711Table(alaw) {
  const t = new Float32Array(256);
  for (let i = 0; i < 256; i++) {
    let v;
    if (alaw) {
      let a = i ^ 0x55;
      const seg = (a & 0x70) >> 4;
      let m = ((a & 0x0f) << 4) + 8;
      if (seg) m = (m + 0x100) << (seg - 1);
      v = a & 0x80 ? m : -m;
    } else {
      const u = ~i & 0xff;
      const seg = (u & 0x70) >> 4;
      const m = ((((u & 0x0f) << 3) + 0x84) << seg) - 0x84;
      v = u & 0x80 ? -m : m;
    }
    t[i] = v / 32768;
  }
  return t;
}

class Player {
  constructor(canvas, camId, { hd = false, onError = () => {} } = {}) {
    this.canvas = canvas;
    this.ctx = canvas.getContext('2d', { alpha: false });
    this.camId = camId;
    this.hd = hd;
    this.onError = onError;
    this.sub = null;
    this.decoder = null;
    this.waitKey = true;
    this.lastTs = null;
    this.clock = 0;
    this.closed = false;
    this.muted = true;
    this.audio = null; // { ctx, decoder | table, nextTime, rate, channels }
  }

  static supported() {
    return typeof window.VideoDecoder === 'function';
  }

  async start() {
    if (!Player.supported()) {
      uiLog('VideoDecoder missing in this webview');
      return this.onError(t('noWebCodecs'));
    }
    const channel = new Channel();
    channel.onmessage = m => this.onMessage(m);
    this.sub = await invoke('subscribe', { id: this.camId, hd: this.hd, sink: channel });
    if (!this.muted) invoke('set_audio', { sub: this.sub, on: true });
    if (this.closed) this.stop();
  }

  setHd(hd) {
    if (hd === this.hd) return;
    this.hd = hd;
    if (this.sub != null) invoke('set_hd', { sub: this.sub, hd });
  }

  stop() {
    this.closed = true;
    this.closeAudio();
    if (this.sub != null) invoke('unsubscribe', { sub: this.sub });
    this.sub = null;
    this.resetDecoder();
  }

  resetDecoder() {
    if (this.decoder && this.decoder.state !== 'closed') {
      try { this.decoder.close(); } catch {}
    }
    this.decoder = null;
    this.waitKey = true;
  }

  onMessage(m) {
    if (this.closed) return;
    if (!this.seen) {
      this.seen = true;
      uiLog(`${this.camId}: first message ${Object.prototype.toString.call(m)} VideoDecoder=${typeof window.VideoDecoder}`);
    }
    const u8 = m instanceof ArrayBuffer ? new Uint8Array(m) : ArrayBuffer.isView(m) ? new Uint8Array(m.buffer, m.byteOffset, m.byteLength) : new Uint8Array(m);
    if (u8[0] === 1) return this.configure(JSON.parse(new TextDecoder().decode(u8.subarray(1))));
    if (u8[0] === 2) return this.decode(u8);
    if (u8[0] === 3) return this.configureAudio(JSON.parse(new TextDecoder().decode(u8.subarray(1))));
    if (u8[0] === 4) return this.decodeAudio(u8.subarray(5));
  }

  /** Sound is off by default; turning it on must come from a user gesture. */
  setMuted(muted) {
    if (muted === this.muted) return;
    this.muted = muted;
    if (muted) this.closeAudio();
    else {
      try {
        const Ctx = window.AudioContext || window.webkitAudioContext;
        this.audioCtx = this.audioCtx || new Ctx();
        this.audioCtx.resume();
      } catch (e) {
        uiLog(`${this.camId}: no AudioContext ${e}`);
      }
    }
    if (this.sub != null) invoke('set_audio', { sub: this.sub, on: !muted });
  }

  closeAudio() {
    const a = this.audio;
    this.audio = null;
    if (a && a.decoder && a.decoder.state !== 'closed') {
      try { a.decoder.close(); } catch {}
    }
    if (this.audioCtx) {
      this.audioCtx.close().catch(() => {});
      this.audioCtx = null;
    }
  }

  async configureAudio(cfg) {
    if (this.muted || !this.audioCtx) return;
    if (this.audio && this.audio.decoder && this.audio.decoder.state !== 'closed') this.audio.decoder.close();
    const a = { nextTime: 0, rate: cfg.sampleRate, channels: cfg.channels };
    if (cfg.codec === 'alaw' || cfg.codec === 'ulaw') {
      a.table = g711Table(cfg.codec === 'alaw');
    } else {
      if (typeof window.AudioDecoder !== 'function') return uiLog(`${this.camId}: no AudioDecoder here`);
      const description = new Uint8Array((cfg.description.match(/../g) || []).map(h => parseInt(h, 16)));
      const config = { codec: cfg.codec, sampleRate: cfg.sampleRate, numberOfChannels: cfg.channels, description };
      a.decoder = new AudioDecoder({
        output: data => this.playAudioData(data),
        error: e => uiLog(`${this.camId}: audio decoder error ${e}`),
      });
      try {
        a.decoder.configure(config);
      } catch (e) {
        return uiLog(`${this.camId}: audio ${cfg.codec} not supported (${e})`);
      }
    }
    this.audio = a;
    uiLog(`${this.camId}: audio ${cfg.codec} ${cfg.sampleRate} Hz`);
  }

  decodeAudio(frame) {
    const a = this.audio;
    if (!a || this.muted) return;
    if (a.table) {
      const pcm = new Float32Array(frame.length);
      for (let i = 0; i < frame.length; i++) pcm[i] = a.table[frame[i]];
      return this.schedule([pcm], a.rate);
    }
    if (!a.decoder || a.decoder.state !== 'configured') return;
    a.ts = (a.ts || 0) + 1;
    try {
      a.decoder.decode(new EncodedAudioChunk({ type: 'key', timestamp: a.ts * 64000, data: frame }));
    } catch (e) {
      uiLog(`${this.camId}: audio decode threw ${e}`);
    }
  }

  playAudioData(data) {
    try {
      const planes = [];
      for (let c = 0; c < data.numberOfChannels; c++) {
        const buf = new Float32Array(data.numberOfFrames);
        data.copyTo(buf, { planeIndex: c, format: 'f32-planar' });
        planes.push(buf);
      }
      this.schedule(planes, data.sampleRate);
    } finally {
      data.close();
    }
  }

  schedule(planes, rate) {
    const ctx = this.audioCtx;
    const a = this.audio;
    if (!ctx || !a || !planes.length || !planes[0].length) return;
    const buffer = ctx.createBuffer(planes.length, planes[0].length, rate);
    planes.forEach((p, i) => buffer.copyToChannel(p, i));
    const now = ctx.currentTime;
    if (a.nextTime < now + 0.01 || a.nextTime > now + AUDIO_MAX_LAG) a.nextTime = now + AUDIO_LEAD;
    const src = ctx.createBufferSource();
    src.buffer = buffer;
    src.connect(ctx.destination);
    src.start(a.nextTime);
    a.nextTime += buffer.duration;
  }

  async configure(cfg) {
    this.resetDecoder();
    const config = { codec: cfg.codec, optimizeForLatency: true, hardwareAcceleration: 'prefer-hardware' };
    // Some engines never answer this (WebKitGTK without a GStreamer H.264 decoder)
    const check = c => Promise.race([VideoDecoder.isConfigSupported(c).then(r => r.supported), wait(2500).then(() => null)]);
    try {
      let ok = await check(config);
      if (ok === false) {
        delete config.hardwareAcceleration;
        ok = await check(config);
      }
      if (ok !== true) {
        uiLog(`${this.camId}: ${cfg.codec} not decodable here (answer: ${ok})`);
        return this.onError(t('unsupportedCodec', cfg.codec));
      }
    } catch {
      /* some engines lack isConfigSupported: just try */
    }
    if (this.closed) return;
    this.decoder = new VideoDecoder({
      output: frame => this.draw(frame),
      error: e => {
        uiLog(`${this.camId}: decoder error ${e}`);
        this.resetDecoder();
      },
    });
    this.decoder.configure(config);
    uiLog(`${this.camId}: configured ${cfg.codec} hw=${config.hardwareAcceleration || 'default'}`);
    this.waitKey = true;
  }

  decode(u8) {
    const d = this.decoder;
    if (!d || d.state !== 'configured') return;
    const key = u8[1] === 1;
    if (this.waitKey && !key) return;
    if (!key && d.decodeQueueSize > MAX_QUEUE) {
      this.waitKey = true; // falling behind: drop to the next keyframe
      return;
    }
    this.waitKey = false;
    const ts = new DataView(u8.buffer, u8.byteOffset + 2, 4).getUint32(0);
    if (this.lastTs !== null) this.clock += ((ts - this.lastTs) >>> 0) / 90; // ms, wraps safely
    this.lastTs = ts;
    try {
      d.decode(new EncodedVideoChunk({ type: key ? 'key' : 'delta', timestamp: Math.round(this.clock * 1000), data: u8.subarray(6) }));
    } catch (e) {
      uiLog(`${this.camId}: decode threw ${e}`);
      this.waitKey = true;
    }
  }

  draw(frame) {
    if (this.closed) return frame.close();
    if (!this.drawn) {
      this.drawn = true;
      uiLog(`${this.camId}: first frame ${frame.displayWidth}x${frame.displayHeight}`);
    }
    const w = frame.displayWidth, h = frame.displayHeight;
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    this.ctx.drawImage(frame, 0, 0, w, h);
    frame.close();
  }
}
