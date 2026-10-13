/**
 * Opera Embed Module - keeps the Win32 child window glued to #embed-container.
 * The embedded Opera keeps its own message loop, so clicks/typing inside it
 * work natively. This file only launches it and syncs position/size.
 *
 * IMPORTANT: the child is a *sibling* of the WebView, painted OVER the HTML.
 * It can't truly live inside the DOM — we just move it to exactly cover
 * #embed-container so it looks docked. Scroll the panel to it before Launch.
 */
(function () {
  'use strict';

  function container() { return document.getElementById('embed-container'); }
  function statusEl() { return document.getElementById('opera-status'); }
  function setStatus(msg) { const el = statusEl(); if (el) el.textContent = msg; }

  async function syncBounds() {
    const el = container();
    if (!el) return;
    const r = el.getBoundingClientRect();
    if (r.width < 2 || r.height < 2) return;
    try {
      const vw = window.innerWidth || 0;
      const vh = window.innerHeight || 0;
      const fn_ = window.TauriBridge && window.TauriBridge.resizeEmbeddedApp
        ? window.TauriBridge.resizeEmbeddedApp.bind(window.TauriBridge)
        : (x, y, w, h, scale, vw_, vh_) => window.__TAURI__.core.invoke('resize_embedded_app',
            { x, y, w, h, scale, vw: vw_, vh: vh_ });
      await fn_(
        Math.round(r.left), Math.round(r.top),
        Math.round(r.width), Math.round(r.height),
        window.devicePixelRatio || 1, vw, vh);
    } catch (e) { console.warn('[opera-embed] resize failed', e); }
  }

  async function launch() {
    const exeInput = document.getElementById('opera-exe-path');
    const exe = exeInput && exeInput.value.trim() ? exeInput.value.trim() : null;
    // Cache the rect BEFORE spawn so the backend can place the window the
    // instant it appears (early resizes used to be dropped).
    await syncBounds();
    // Scroll the container into view — the child can't scroll with the page.
    container()?.scrollIntoView({ block: 'nearest' });
    setStatus('Launching Opera…');
    try {
      const msg = window.TauriBridge && window.TauriBridge.launchOperaEmbed
        ? await window.TauriBridge.launchOperaEmbed(exe)
        : await window.__TAURI__.core.invoke('launch_opera_embed', { exe });
      setStatus(typeof msg === 'string' ? msg : 'Opera launched — docking…');
      const ph = document.getElementById('embed-placeholder');
      if (ph) ph.style.display = 'none';
      // Opera cold-starts slowly; keep pushing bounds until it docks.
      // Backend also re-applies `pending` on embed + listens for
      // `opera-embedded`, so this is belt-and-suspenders.
      let n = 0;
      const timer = setInterval(async () => {
        await syncBounds();
        if (++n >= 30) clearInterval(timer); // 30 x 500ms = 15s
      }, 500);
    } catch (e) {
      console.error('[opera-embed] launch failed', e);
      setStatus('Launch failed: ' + (e && e.message ? e.message : e));
    }
  }

  async function close() {
    try {
      if (window.TauriBridge && window.TauriBridge.closeEmbeddedApp) {
        await window.TauriBridge.closeEmbeddedApp();
      } else {
        await window.__TAURI__.core.invoke('close_embedded_app');
      }
      setStatus('Not embedded');
      const ph = document.getElementById('embed-placeholder');
      if (ph) ph.style.display = 'flex';
    } catch (e) { setStatus('Close failed: ' + e); }
  }

  // Dock the extra app spawned from the Settings list, after the user has
  // clicked through its launchers/logins. Opera's own Launch still
  // auto-docks; this is only for the two-phase extra-app flow.
  async function embedApp() {
    await syncBounds();
    container()?.scrollIntoView({ block: 'nearest' });
    setStatus('Docking…');
    try {
      const msg = window.TauriBridge && window.TauriBridge.embedLaunchedApp
        ? await window.TauriBridge.embedLaunchedApp()
        : await window.__TAURI__.core.invoke('embed_launched_app');
      setStatus(typeof msg === 'string' ? msg : 'Docked — docking…');
      const ph = document.getElementById('embed-placeholder');
      if (ph) ph.style.display = 'none';
      await syncBounds();
      setTimeout(syncBounds, 400);
    } catch (e) {
      console.error('[opera-embed] embed failed', e);
      setStatus('Embed failed: ' + (e && e.message ? e.message : e));
    }
  }

  async function prefillExePath() {
    try {
      const invoke = window.__TAURI__?.core?.invoke;
      if (!invoke) return;
      const found = window.TauriBridge?.findOperaExe
        ? await window.TauriBridge.findOperaExe()
        : await invoke('find_opera_exe');
      if (found) {
        const input = document.getElementById('opera-exe-path');
        if (input && !input.value) {
          input.value = found;
          input.placeholder = found;
        }
        setStatus('Found Opera at ' + found + ' — click Launch.');
      }
    } catch (_) {}
  }

  function init() {
    document.getElementById('opera-launch')?.addEventListener('click', launch);
    document.getElementById('opera-embed-app')?.addEventListener('click', embedApp);
    document.getElementById('opera-close')?.addEventListener('click', close);
    prefillExePath();
    // Prime the pending rect so first launch already knows where to dock.
    setTimeout(syncBounds, 500);
    const el = container();
    if (el) {
      new ResizeObserver(() => syncBounds()).observe(el);
      // Clicking the container gives native focus to Opera.
      el.addEventListener('mousedown', () => {
        try {
          const p = window.TauriBridge && window.TauriBridge.focusEmbeddedApp
            ? window.TauriBridge.focusEmbeddedApp()
            : window.__TAURI__.core.invoke('focus_embedded_app');
          if (p && p.catch) p.catch(() => {});
        } catch (_) {}
      });
    }
    window.addEventListener('resize', syncBounds);
    window.addEventListener('scroll', syncBounds, true);
    // Backend notifies the moment SetParent succeeds — dock immediately.
    try {
      window.__TAURI__?.event?.listen('opera-embedded', async () => {
        setStatus('Opera docked — click inside to control it.');
        await syncBounds();
        setTimeout(syncBounds, 400);
      });
      window.__TAURI__?.event?.listen('opera-closed', () => {
        setStatus('Opera closed.');
        const ph = document.getElementById('embed-placeholder');
        if (ph) ph.style.display = 'flex';
      });
    } catch (_) {}
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else { init(); }
})();
