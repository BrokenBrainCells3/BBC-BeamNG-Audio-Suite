/**
 * Extra Apps Module - manage additional application exe paths from the
 * Settings panel. UI only for now: add via the plus button, listed under
 * it with a generic icon, edit button per row. Persisted in localStorage.
 */
(function () {
  'use strict';

  const STORE_KEY = 'bbc.extraApps.v1';

  const ICON_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="12" rx="2"/><line x1="8" y1="20" x2="16" y2="20"/><line x1="12" y1="16" x2="12" y2="20"/></svg>';

  let editingId = null;
  const iconCache = new Map(); // exe path -> data URL (null = none, keep glyph)

  function esc(s) {
    return String(s == null ? '' : s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function load() {
    try {
      const raw = localStorage.getItem(STORE_KEY);
      if (!raw) return [];
      const arr = JSON.parse(raw);
      return Array.isArray(arr) ? arr.filter((a) => a && typeof a.exe === 'string') : [];
    } catch (_) {
      return [];
    }
  }

  function save(apps) {
    try {
      localStorage.setItem(STORE_KEY, JSON.stringify(apps));
    } catch (_) {}
  }

  function displayName(exe) {
    const base = String(exe || '').split(/[\\/]/).pop() || 'app';
    return base.replace(/\.exe$/i, '') || 'app';
  }

  function listEl() { return document.getElementById('extra-apps-list'); }
  function editorEl() { return document.getElementById('extra-app-editor'); }
  function exeEl() { return document.getElementById('extra-app-exe'); }

  function render() {
    const ul = listEl();
    if (!ul) return;
    const apps = load();
    if (!apps.length) {
      ul.innerHTML = '<li class="extra-apps-empty">No extra apps yet — click + Add Application.</li>';
      return;
    }
    ul.innerHTML = apps.map((a) => {
      const args = typeof a.args === 'string' ? a.args : '';
      const mcpEnabled = a.mcpEnabled ? ' <span class="mcp-badge">MCP</span>' : '';
      return '<li class="extra-app-row" data-id="' + esc(a.id) + '">' +
        '<span class="extra-app-icon" data-exe="' + esc(a.exe) + '">' + ICON_SVG + '</span>' +
        '<span class="extra-app-text">' +
          '<span class="extra-app-name">' + esc(displayName(a.exe)) + '</span>' +
          '<span class="extra-app-path" title="' + esc(a.exe) + '">' + esc(a.exe) + '</span>' +
          (args ? '<span class="extra-app-args" title="' + esc(args) + '">' + esc(args) + '</span>' : '') +
          (mcpEnabled) +
        '</span>' +
        '<button class="btn-secondary btn-small extra-app-launch" data-id="' + esc(a.id) + '" data-exe="' + esc(a.exe) + '" data-args="' + esc(args) + '" aria-label="Launch and embed">Launch</button>' +
        '<button class="btn-ghost btn-small extra-app-edit" data-id="' + esc(a.id) + '" aria-label="Edit exe path">Edit</button>' +
      '</li>';
    }).join('');
    hydrateIcons(ul);
  }

  // Swap the generic glyph for each exe's real icon (backend extraction).
  // Failures keep the glyph; results are cached per path for the session.
  async function hydrateIcons(root) {
    if (!window.TauriBridge || !window.TauriBridge.getExeIcon) return;
    const spots = (root || document).querySelectorAll('.extra-app-icon[data-exe]');
    for (const s of spots) {
      const exe = s.getAttribute('data-exe');
      if (!exe || s.dataset.done) continue;
      s.dataset.done = '1';
      try {
        let url = iconCache.has(exe) ? iconCache.get(exe) : undefined;
        if (url === undefined) {
          url = await window.TauriBridge.getExeIcon(exe);
          iconCache.set(exe, url || null);
        }
        if (url) s.innerHTML = '<img alt="" src="' + url + '">';
      } catch (_) {}
    }
  }

  function argsEl() { return document.getElementById('extra-app-args'); }
  function mcpEnabledEl() { return document.getElementById('extra-app-mcp-enabled'); }
  function mcpHostEl() { return document.getElementById('extra-app-mcp-host'); }
  function mcpPortEl() { return document.getElementById('extra-app-mcp-port'); }

  function openEditor(id) {
    editingId = id || null;
    const input = exeEl();
    const argsInput = argsEl();
    const mcpEnabledInput = mcpEnabledEl();
    const mcpHostInput = mcpHostEl();
    const mcpPortInput = mcpPortEl();
    if (id) {
      const found = load().find((a) => a.id === id);
      if (input) input.value = found ? found.exe : '';
      if (argsInput) argsInput.value = found && typeof found.args === 'string' ? found.args : '';
      if (mcpEnabledInput) mcpEnabledInput.checked = !!found?.mcpEnabled;
      if (mcpHostInput) mcpHostInput.value = found?.mcpHost || '127.0.0.1';
      if (mcpPortInput) mcpPortInput.value = found?.mcpPort || '29292';
    } else {
      if (input) input.value = '';
      if (argsInput) argsInput.value = '';
      if (mcpEnabledInput) mcpEnabledInput.checked = false;
      if (mcpHostInput) mcpHostInput.value = '127.0.0.1';
      if (mcpPortInput) mcpPortInput.value = '29292';
    }
    editorEl()?.classList.remove('hidden');
    input?.focus();
  }

  function closeEditor() {
    editingId = null;
    const input = exeEl();
    if (input) input.value = '';
    const argsInput = argsEl();
    if (argsInput) argsInput.value = '';
    const mcpEnabledInput = mcpEnabledEl();
    if (mcpEnabledInput) mcpEnabledInput.checked = false;
    const mcpHostInput = mcpHostEl();
    if (mcpHostInput) mcpHostInput.value = '127.0.0.1';
    const mcpPortInput = mcpPortEl();
    if (mcpPortInput) mcpPortInput.value = '29292';
    editorEl()?.classList.add('hidden');
  }

  function onSave() {
    const input = exeEl();
    const exe = input ? input.value.trim() : '';
    if (!exe) {
      if (window.Utils && window.Utils.showNotification) {
        window.Utils.showNotification('Exe path cannot be empty', 'error');
      }
      input?.focus();
      return;
    }
    const argsInput = argsEl();
    const args = argsInput ? argsInput.value.trim() : '';
    const mcpEnabledInput = mcpEnabledEl();
    const mcpHostInput = mcpHostEl();
    const mcpPortInput = mcpPortEl();
    const mcpEnabled = mcpEnabledInput ? mcpEnabledInput.checked : false;
    const mcpHost = mcpHostInput ? mcpHostInput.value.trim() : '127.0.0.1';
    const mcpPort = mcpPortInput ? parseInt(mcpPortInput.value, 10) : 29292;
    const apps = load();
    if (editingId) {
      const found = apps.find((a) => a.id === editingId);
      if (found) {
        found.exe = exe;
        if (args) found.args = args;
        else delete found.args;
        found.mcpEnabled = mcpEnabled;
        found.mcpHost = mcpHost;
        found.mcpPort = mcpPort;
      }
    } else {
      const item = { id: 'app-' + Date.now().toString(36), exe };
      if (args) item.args = args;
      if (mcpEnabled) item.mcpEnabled = true;
      item.mcpHost = mcpHost;
      item.mcpPort = mcpPort;
      apps.push(item);
    }
    save(apps);
    render();
    closeEditor();
  }

  function init() {
    document.getElementById('extra-app-add')?.addEventListener('click', () => openEditor(null));
    document.getElementById('extra-app-save')?.addEventListener('click', onSave);
    document.getElementById('extra-app-cancel')?.addEventListener('click', closeEditor);
    document.getElementById('extra-app-mcp-test')?.addEventListener('click', onTestMcpConnection);
    exeEl()?.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') onSave();
      else if (e.key === 'Escape') closeEditor();
    });
    listEl()?.addEventListener('click', (e) => {
      if (!e.target || !e.target.closest) return;
      const editBtn = e.target.closest('.extra-app-edit');
      if (editBtn && editBtn.dataset && editBtn.dataset.id) {
        openEditor(editBtn.dataset.id);
        return;
      }
      const launchBtn = e.target.closest('.extra-app-launch');
      if (launchBtn && launchBtn.dataset && launchBtn.dataset.exe) {
        onLaunch(launchBtn, launchBtn.dataset.exe, launchBtn.dataset.args || '', launchBtn.dataset.id);
      }
    });
    render();
  }

  // Test MCP connection
  async function onTestMcpConnection() {
    const host = mcpHostEl()?.value?.trim() || '127.0.0.1';
    const port = parseInt(mcpPortEl()?.value || '29292', 10);
    const btn = document.getElementById('extra-app-mcp-test');
    if (!btn) return;
    btn.disabled = true;
    btn.textContent = 'Testing...';
    try {
      if (window.TauriBridge && window.TauriBridge.testMcpConnection) {
        const result = await window.TauriBridge.testMcpConnection(host, port);
        if (window.Utils && window.Utils.showNotification) {
          window.Utils.showNotification('MCP: ' + result, 'success');
        }
      } else {
        throw new Error('MCP test function not available');
      }
    } catch (err) {
      const text = err && err.message ? err.message : (typeof err === 'string' ? err : 'Test failed');
      if (window.Utils && window.Utils.showNotification) {
        window.Utils.showNotification('MCP Test: ' + text, 'error');
      }
    } finally {
      btn.disabled = false;
      btn.textContent = 'Test MCP Connection';
    }
  }

  // Spawn the exe as a normal window (click through launchers/logins),
  // then dock it with the browser panel's Embed App button and broadcast
  // it from the normal Stream panel. Close undocks whatever is embedded.
  async function onLaunch(btn, exe, args, appId) {
    if (!window.TauriBridge || !window.TauriBridge.launchEmbeddedApp) return;
    btn.disabled = true;
    try {
      // Get MCP settings from the app data
      const apps = load();
      const app = apps.find(a => a.id === appId);
      const mcpEnabled = app?.mcpEnabled || false;
      const mcpHost = app?.mcpHost || '127.0.0.1';
      const mcpPort = app?.mcpPort || 29292;
      
      const msg = await window.TauriBridge.launchEmbeddedApp(exe, args, appId, mcpEnabled, mcpHost, mcpPort);
      if (window.Utils && window.Utils.showNotification) {
        window.Utils.showNotification(typeof msg === 'string' ? msg : 'Launched — docking…', 'success');
      }
    } catch (err) {
      const text = err && err.message ? err.message
        : (typeof err === 'string' ? err : 'Launch failed');
      if (window.Utils && window.Utils.showNotification) {
        window.Utils.showNotification(text, 'error');
      } else {
        console.error('[extra-apps] launch failed', err);
      }
    } finally {
      btn.disabled = false;
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else { init(); }
})();
