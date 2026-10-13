/**
 * Opera Stream Module - Start/Stop JPEG streaming of the embedded Opera window.
 * Single streaming method: the app mirrors every encoded frame to disk
 * snapshot files inside the BeamNG mods folder; vehicle texture pages poll
 * them via local:// with a cache-buster. The preview below polls the latest
 * in-memory frame directly from Rust (no network involved).
 */
(function () {
  'use strict';

  let previewTimer = null;
  let statusTimer = null;
  let frameCount = 0;

  function statusEl() { return document.getElementById('stream-status'); }
  function setStatus(msg) { const el = statusEl(); if (el) el.textContent = msg; }
  function vals() {
    const f = parseInt((document.getElementById('stream-fps') || {}).value, 10);
    const q = parseInt((document.getElementById('stream-quality') || {}).value, 10);
    return {
      fps: Number.isFinite(f) ? Math.min(120, Math.max(5, f)) : 60,
      quality: Number.isFinite(q) ? Math.min(95, Math.max(10, q)) : 70
    };
  }

  function stopPreview() {
    if (previewTimer) { clearInterval(previewTimer); previewTimer = null; }
  }

  // Preview: poll the in-memory latest frame from Rust (~15fps is plenty for
  // a preview; the disk snapshot pipeline runs at full capture fps).
  function startPreview() {
    stopPreview();
    const img = document.getElementById('stream-preview');
    const ph = document.getElementById('stream-preview-placeholder');
    if (ph) ph.style.display = 'none';
    if (!img) return;
    frameCount = 0;
    let busy = false;
    async function pull() {
      if (busy) return;
      busy = true;
      try {
        const invoke = window.__TAURI__.core.invoke;
        const b64 = await invoke('preview_frame_b64');
        if (b64) {
          frameCount++;
          img.src = 'data:image/jpeg;base64,' + b64;
          if (frameCount === 1 || frameCount % 90 === 0) {
            setStatus('Streaming (local:// snapshots) — preview frame ' + frameCount);
          }
        }
      } catch (_) {}
      busy = false;
    }
    previewTimer = setInterval(pull, 66);
    pull();
  }

  async function refreshStatus() {
    try {
      const invoke = window.__TAURI__.core.invoke;
      const s = window.TauriBridge && window.TauriBridge.operaStreamStatus
        ? await window.TauriBridge.operaStreamStatus()
        : await invoke('stream_status');
      if (s && s.running) {
        let extra = '';
        if (s.mode) {
          extra = ' [' + s.mode + ' frame ' + (s.frame_w || 0) + 'x' + (s.frame_h || 0) +
            ' box ' + (s.box_w || 0) + 'x' + (s.box_h || 0) + ']';
        }
        setStatus('Live (local:// snapshots) — ' + (s.frames || 0) + ' frames @ ' + (s.fps || 0) + 'fps' + extra);
      }
    } catch (_) {}
  }

  async function start() {
    const { fps, quality } = vals();
    setStatus('Starting stream… (needs Opera docked above)');
    try {
      const invoke = window.__TAURI__.core.invoke;
      const msg = window.TauriBridge && window.TauriBridge.startOperaStream
        ? await window.TauriBridge.startOperaStream(0, fps, quality)
        : await invoke('start_opera_stream', { port: 0, fps, quality });
      setStatus(typeof msg === 'string' ? msg : 'Stream started (local:// snapshots)');
      startPreview();
      if (!statusTimer) statusTimer = setInterval(refreshStatus, 2000);
    } catch (e) {
      console.error('[opera-stream] start failed', e);
      setStatus('Start failed: ' + (e && e.message ? e.message : e) + ' — dock Opera first.');
    }
  }

  async function stop() {
    try {
      const invoke = window.__TAURI__.core.invoke;
      if (window.TauriBridge && window.TauriBridge.stopOperaStream) {
        await window.TauriBridge.stopOperaStream();
      } else {
        await invoke('stop_opera_stream');
      }
    } catch (_) {}
    stopPreview();
    setStatus('Stream stopped');
    if (statusTimer) { clearInterval(statusTimer); statusTimer = null; }
    const ph = document.getElementById('stream-preview-placeholder');
    if (ph) ph.style.display = 'flex';
  }

  function init() {
    document.getElementById('stream-start')?.addEventListener('click', start);
    document.getElementById('stream-stop')?.addEventListener('click', stop);
    try {
      window.__TAURI__?.event?.listen('stream-started', () => {
        startPreview();
        if (!statusTimer) statusTimer = setInterval(refreshStatus, 2000);
      });
      window.__TAURI__?.event?.listen('stream-stopped', () => {
        stopPreview();
        setStatus('Stream stopped');
      });
    } catch (_) {}
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else { init(); }
})();