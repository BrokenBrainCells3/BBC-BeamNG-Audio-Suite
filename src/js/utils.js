/**
 * Utils Module - Pure utility functions
 * No DOM access, no Tauri API calls, no side effects
 * Safe to load first before anything else exists on the page
 */

(function() {
  'use strict';

  /**
   * Convert hex color to HSL
   */
  const hexToHsl = (hex) => {
    let r = parseInt(hex.slice(1, 3), 16) / 255;
    let g = parseInt(hex.slice(3, 5), 16) / 255;
    let b = parseInt(hex.slice(5, 7), 16) / 255;

    let max = Math.max(r, g, b), min = Math.min(r, g, b);
    let h, s;
    const l = (max + min) / 2;

    if (max === min) {
      h = s = 0;
    } else {
      const d = max - min;
      s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
      switch (max) {
        case r: h = ((g - b) / d + (g < b ? 6 : 0)) / 6; break;
        case g: h = ((b - r) / d + 2) / 6; break;
        case b: h = ((r - g) / d + 4) / 6; break;
      }
    }

    return { h: Math.round(h * 360), s, l };
  };

  /**
   * Convert hex color to RGBA string
   */
  const hexToRgba = (hex, alpha) => {
    const r = parseInt(hex.slice(1, 3), 16);
    const g = parseInt(hex.slice(3, 5), 16);
    const b = parseInt(hex.slice(5, 7), 16);
    return `rgba(${r}, ${g}, ${b}, ${alpha})`;
  };

  /**
   * Adjust color brightness by percent
   */
  const adjustColorBrightness = (hex, percent) => {
    const num = parseInt(hex.replace("#", ""), 16);
    const amt = Math.round(2.55 * percent);
    const R = Math.max(0, Math.min(255, (num >> 16) + amt));
    const G = Math.max(0, Math.min(255, ((num >> 8) & 0x00FF) + amt));
    const B = Math.max(0, Math.min(255, (num & 0x0000FF) + amt));
    return `#${(1 << 24 | R << 16 | G << 8 | B).toString(16).slice(1)}`;
  };

  /**
   * Format time in milliseconds to MM:SS
   */
  const formatTime = (ms) => {
    const totalSeconds = Math.floor(ms / 1000);
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = totalSeconds % 60;
    return `${minutes}:${seconds.toString().padStart(2, '0')}`;
  };

  /**
   * Convert radians to degrees
   */
  const radiansToDegrees = (rad) => {
    if (typeof rad !== 'number' || isNaN(rad)) return 0;
    return rad * (180 / Math.PI);
  };

  /**
   * Escape HTML special characters
   */
  const escapeHtml = (text) => {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
  };

  /**
   * Sanitize WebSocket URL - only allow ws:// and wss:// protocols
   */
  const sanitizeWebSocketUrl = (url) => {
    const sanitized = url.trim().replace(/[<>"'&]/g, '');
    if (!sanitized.match(/^wss?:\/\//)) {
      return 'ws://' + sanitized;
    }
    return sanitized;
  };

  /**
   * Validate WebSocket URL format
   */
  const validateWebSocketUrl = (url) => {
    try {
      const parsed = new URL(url);
      return parsed.protocol === 'ws:' || parsed.protocol === 'wss:';
    } catch {
      return false;
    }
  };

  /**
   * Validate Discord webhook URL - must be HTTPS from discord.com or discordapp.com
   */
  const validateDiscordWebhookUrl = (url) => {
    try {
      const parsed = new URL(url);
      return parsed.protocol === 'https:' &&
             (parsed.hostname === 'discord.com' ||
              parsed.hostname === 'discordapp.com' ||
              parsed.hostname.endsWith('.discord.com'));
    } catch {
      return false;
    }
  };

  /**
   * Sanitize error messages - strip internal paths and stack traces
   */
  const sanitizeErrorMessage = (message) => {
    if (typeof message !== 'string') return String(message);
    return message
      .replace(/at\s+.*\n/g, '')
      .replace(/at\s+.*\(/g, '')
      .replace(/\\[^\s\\]+/g, '[path]')
      .replace(/\/[^\s/]+/g, '[path]')
      .replace(/C:\\[^\s]+/gi, '[path]')
      .trim();
  };

  /**
   * Safe text content setter
   */
  const safeSetText = (id, text) => {
    const el = document.getElementById(id);
    if (el) el.textContent = text;
  };

  /**
   * Format gear value
   */
  const formatGear = (gear) => {
    if (gear === undefined || gear === null) return '-';
    const g = typeof gear === 'number' ? gear : parseInt(gear, 10);
    if (g === 0) return 'R';
    if (g === 1) return 'N';
    return (g - 1).toString();
  };

  /**
   * Invoke with timeout to prevent indefinite blocking
   */
  const invokeWithTimeout = async (invokeFn, command, args = {}, timeout = 5000) => {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), timeout);

    try {
      const result = await Promise.race([
        invokeFn(command, args),
        new Promise((_, reject) => {
          controller.signal.addEventListener('abort', () => {
            reject(new Error(`Command '${command}' timed out after ${timeout}ms`));
          });
        })
      ]);
      clearTimeout(timeoutId);
      return result;
    } catch (error) {
      clearTimeout(timeoutId);
      if (error.message.includes('timed out')) {
        throw error;
      }
      throw error;
    }
  };

  /**
   * Show a toast notification
   */
  const showNotification = (message, type = 'info', duration) => {
    const container = document.getElementById('notification-container');
    if (!container) {
      return;
    }

    const icons = {
      success: `<svg class="notification-icon" stroke="currentColor" viewBox="0 0 24 24" fill="none"><path d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/></svg>`,
      info: `<svg class="notification-icon" stroke="currentColor" viewBox="0 0 24 24" fill="none"><path d="M13 16h-1v-4h1m0-4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/></svg>`,
      warning: `<svg class="notification-icon" stroke="currentColor" viewBox="0 0 24 24" fill="none"><path d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/></svg>`,
      error: `<svg class="notification-icon" stroke="currentColor" viewBox="0 0 24 24" fill="none"><path d="M10 14l2-2m0 0l2-2m-2 2l-2-2m2 2l2 2m7-2a9 9 0 11-18 0 9 9 0 0118 0z" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/></svg>`
    };

    const autoDismiss = type !== 'error';
    const autoDismissDuration = duration !== undefined ? duration : (type === 'error' ? 0 : 5000);

    const notification = document.createElement('div');
    notification.className = `notification ${type}`;
    notification.role = 'alert';

    notification.innerHTML = `
      ${icons[type] || icons.info}
      <p class="notification-message">${escapeHtml(message)}</p>
      <button class="notification-close" aria-label="Close">
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M18 6L6 18M6 6l12 12"/>
        </svg>
      </button>
    `;

    const closeBtn = notification.querySelector('.notification-close');
    const removeNotification = () => {
      notification.classList.add('sliding-out');
      notification.addEventListener('animationend', () => notification.remove());
    };

    closeBtn.addEventListener('click', removeNotification);

    const timeoutId = autoDismiss ? setTimeout(removeNotification, autoDismissDuration) : null;

    notification.addEventListener('click', (e) => {
      if (e.target !== closeBtn && !closeBtn.contains(e.target)) {
        if (timeoutId) clearTimeout(timeoutId);
        removeNotification();
      }
    });

    container.appendChild(notification);
  };

  /**
   * Get user name from localStorage
   */
  const getUserName = () => localStorage.getItem('audio-viz-user-name');

  // ============================================
  // LOG THROTTLE UTILITY
  // ============================================
  // Throttles repeating logs to fire no more than once every 3-5 seconds
  // Uses a random delay between 3000-5000ms to avoid log clustering
  // ============================================
  
  const logThrottleState = new Map(); // key -> { lastLogTime, minDelay, maxDelay }
  const logHistory = []; // Store all logs for export
  const MAX_LOG_HISTORY = 1000; // Keep last 1000 logs
  
  /**
   * Throttled logging utility for repeating logs
   * @param {string} key - Unique identifier for this log (e.g., 'band_subwoofer', 'voice_connect')
   * @param {Function} logFn - The console function to call (console.log, console.error, etc.)
   * @param {any[]} args - Arguments to pass to the log function
   * @param {number} minDelay - Minimum delay between logs in ms (default: 3000)
   * @param {number} maxDelay - Maximum delay between logs in ms (default: 5000)
   */
  const throttleLog = (key, logFn, args, minDelay = 3000, maxDelay = 5000) => {
    const now = Date.now();
    const state = logThrottleState.get(key);
    
    // Randomize delay on first call
    if (!state) {
      const randomDelay = Math.floor(Math.random() * (maxDelay - minDelay + 1)) + minDelay;
      logThrottleState.set(key, { lastLogTime: now, minDelay, maxDelay, randomDelay });
      logFn(...args);
      return true;
    }
    
    // Check if enough time has passed
    const elapsed = now - state.lastLogTime;
    if (elapsed >= state.randomDelay) {
      state.lastLogTime = now;
      // Generate new random delay for next occurrence
      state.randomDelay = Math.floor(Math.random() * (state.maxDelay - state.minDelay + 1)) + state.minDelay;
      logFn(...args);
      return true;
    }
    
    return false; // Log was throttled
  };
  
  /**
   * Initialize log panel at bottom of app
   * Called once on app init
   */
  const initLogPanel = () => {
    // Check if already initialized
    if (logPanelInitialized) return;
    logPanelInitialized = true;
    
    // Get container from DOM
    const container = document.getElementById('log-container');
    if (!container) {
      console.warn('[LogPanel] Container not found in DOM');
      return;
    }
    
    // Clear empty state
    const emptyState = container.querySelector('.log-empty-state');
    if (emptyState) {
      emptyState.style.display = 'none';
    }
    
    // Setup button listeners
    const clearBtn = document.getElementById('log-panel-clear');
    if (clearBtn) {
      clearBtn.addEventListener('click', () => {
        container.innerHTML = '';
        logHistory.length = 0;
        logToUI('info', 'Log panel cleared');
        updateLogCount();
      });
    }
    
    const exportBtn = document.getElementById('log-panel-export');
    if (exportBtn) {
      exportBtn.addEventListener('click', () => {
        exportLogs();
      });
    }
    
    logToUI('info', 'Log panel initialized');
  };
  
  let logPanelInitialized = false;
  
  /**
   * Update log count display
   */
  const updateLogCount = () => {
    const container = document.getElementById('log-container');
    if (container) {
      const countEl = container.querySelector('.log-count-display');
      if (countEl) {
        countEl.textContent = logHistory.length;
      }
    }
  };
  
  /**
   * Log to browser UI panel at bottom of app
   * @param {string} type - Log type: 'log', 'error', 'warn', 'info', 'success'
   * @param {string} message - Log message
   * @param {any} [data] - Optional data to display
   */
  const logToUI = (type, message, data) => {
    // Initialize panel if not already done
    if (!logPanelInitialized) {
      initLogPanel();
    }
    
    // Store in history
    const logEntry = {
      timestamp: new Date().toISOString(),
      type,
      message,
      data: data !== undefined ? (typeof data === 'object' ? JSON.stringify(data) : String(data)) : null
    };
    logHistory.push(logEntry);
    if (logHistory.length > MAX_LOG_HISTORY) {
      logHistory.shift(); // Remove oldest
    }
    
    // Get container
    const container = document.getElementById('log-container');
    if (!container) return;
    
    // Create log entry element
    const timestamp = new Date().toLocaleTimeString();
    const logEl = document.createElement('div');
    logEl.className = `log-entry ${type}`;
    
    const typeIcon = '';
    
    logEl.innerHTML = `
      <span class="log-timestamp">[${timestamp}]</span>
      <span class="log-icon">${typeIcon}</span>
      <span class="log-message">${escapeHtml(message)}</span>
    `;
    
    if (data !== undefined) {
      const dataEl = document.createElement('div');
      dataEl.className = 'log-data';
      try {
        dataEl.textContent = typeof data === 'object' ? JSON.stringify(data, null, 2) : String(data);
      } catch {
        dataEl.textContent = '[Object]';
      }
      logEl.appendChild(dataEl);
    }
    
    container.appendChild(logEl);
    container.scrollTop = container.scrollHeight;

    // Auto-hide old logs if too many
    while (container.children.length > 200) {
      const firstChild = container.firstChild;
      if (firstChild && firstChild.classList && firstChild.classList.contains('log-entry')) {
        container.removeChild(firstChild);
      } else {
        break;
      }
    }
  };
  
  /**
   * Export logs to JSON file
   */
  const exportLogs = () => {
    if (logHistory.length === 0) {
      window.Utils.showNotification('No logs to export', 'warning');
      return;
    }
    
    const exportData = {
      exportTime: new Date().toISOString(),
      totalLogs: logHistory.length,
      logs: logHistory
    };
    
    const blob = new Blob([JSON.stringify(exportData, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `audio-app-logs-${new Date().toISOString().replace(/[:.]/g, '-')}.json`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
    
    window.Utils.showNotification(`Exported ${logHistory.length} logs`, 'success');
    logToUI('success', `Logs exported to file (${logHistory.length} entries)`);
  };

  // Expose all utility functions globally
  window.Utils = {
    hexToHsl,
    hexToRgba,
    adjustColorBrightness,
    formatTime,
    radiansToDegrees,
    escapeHtml,
    sanitizeWebSocketUrl,
    validateWebSocketUrl,
    validateDiscordWebhookUrl,
    sanitizeErrorMessage,
    safeSetText,
    formatGear,
    invokeWithTimeout,
    showNotification,
    getUserName,
    throttleLog,
    logToUI,
    exportLogs,
    initLogPanel
  };

  // Wrap all DOM queries in DOMContentLoaded
  document.addEventListener('DOMContentLoaded', () => {
    // Your DOM-related code here
  });

})();
