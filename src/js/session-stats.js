/**
 * Session Stats Module - Track and display fun session statistics
 * Uses window.TauriBridge for all Tauri API calls
 * Uses window.Utils for utility functions
 */

(function() {
  'use strict';

  // Session Stats State
  let sessionStats = null;
  let lifetimeStats = null;
  let sessionLogFiles = [];
  let currentSessionInterval = null;
  let sessionStartTime = null;
  let statsUpdateInterval = null;
  let lifetimeStatsInterval = null;
  let statsObserver = null;

  /**
   * Initialize session stats panel
   */
  const initSessionStats = async () => {
    await loadSessionStats();
    await loadLifetimeStats();
    await loadSessionLogFiles();
    setupEventListeners();
    startSessionTimer();
    
    const panel = document.getElementById('session-stats-panel');
    if (panel) {
      statsObserver = new IntersectionObserver((entries) => {
        entries[0].isIntersecting ? startStatsPolling() : stopStatsPolling();
      });
      statsObserver.observe(panel);
    } else {
      startStatsPolling();
    }
  };

  /**
   * Load current session stats
   */
  const loadSessionStats = async () => {
    try {
      sessionStats = await window.TauriBridge.getSessionStats();
      renderLiveSessionCard();
    } catch (error) {
      console.error('[SessionStats] Failed to load session stats:', error);
      window.Utils.logToUI('error', 'Failed to load session stats: ' + window.Utils.sanitizeErrorMessage(error));
    }
  };

  /**
   * Load lifetime stats
   */
  const loadLifetimeStats = async () => {
    try {
      lifetimeStats = await window.TauriBridge.getLifetimeStats();
      renderLifetimeStatsGrid();
    } catch (error) {
      console.error('[SessionStats] Failed to load lifetime stats:', error);
      window.Utils.logToUI('error', 'Failed to load lifetime stats: ' + window.Utils.sanitizeErrorMessage(error));
    }
  };

  /**
   * Load session log files list
   */
  const loadSessionLogFiles = async () => {
    try {
      sessionLogFiles = await window.TauriBridge.getSessionLogFiles();
      populateLogSelector();
    } catch (error) {
      console.error('[SessionStats] Failed to load log files:', error);
      window.Utils.logToUI('error', 'Failed to load log files: ' + window.Utils.sanitizeErrorMessage(error));
    }
  };

  /**
   * Render live session card
   */
  const renderLiveSessionCard = () => {
    const container = document.querySelector('.session-live-card');
    if (!container) {
      console.warn('[SessionStats] .session-live-card container not found');
      return;
    }

    if (!sessionStats) {
      console.warn('[SessionStats] No sessionStats data to render');
      container.innerHTML = '<div class="session-empty-state"><p>Loading session data...</p></div>';
      return;
    }

    const sessionDuration = calculateSessionDuration();
    const crashesCount = sessionStats.crashes_detected ? sessionStats.crashes_detected.length : 0;

    container.innerHTML = `
      <div class="live-session-header">
        <h3>
          Live Session Stats
        </h3>
        <div class="live-session-indicator">
          <span class="live-dot"></span>
          <span>Recording</span>
        </div>
      </div>
      <div class="live-stats-grid">
        <div class="live-stat-item">
          <div class="live-stat-value" id="live-session-duration">${formatDuration(sessionDuration)}</div>
          <div class="live-stat-label">Session Duration</div>
        </div>
        <div class="live-stat-item">
          <div class="live-stat-value" id="live-session-distance">${sessionStats.distance_traveled_km.toFixed(2)} km</div>
          <div class="live-stat-label">Distance This Session</div>
        </div>
        <div class="live-stat-item">
          <div class="live-stat-value" id="live-session-crashes">${crashesCount}</div>
          <div class="live-stat-label">Crashes This Session</div>
        </div>
        <div class="live-stat-item">
          <div class="live-stat-value" id="live-session-top-speed">${sessionStats.top_speed_this_session.toFixed(1)} km/h</div>
          <div class="live-stat-label">Top Speed This Session</div>
        </div>
      </div>
    `;
  };

  /**
   * Render lifetime stats grid
   */
  const renderLifetimeStatsGrid = () => {
    const container = document.getElementById('lifetime-stats-grid');
    if (!container) return;

    if (!lifetimeStats) {
      container.innerHTML = '<div class="session-empty-state"><p>Loading lifetime data...</p></div>';
      return;
    }

    const statCards = [
      // Core Stats
      { value: formatDistance(lifetimeStats.total_distance_km || 0), label: 'Total Distance' },
      { value: `${(lifetimeStats.all_time_top_speed || 0).toFixed(1)} km/h`, label: 'All-Time Top Speed' },
      { value: formatDuration(lifetimeStats.total_drive_time_seconds || 0), label: 'Total Drive Time' },
      { value: lifetimeStats.total_crashes || 0, label: 'Total Crashes' },
      { value: lifetimeStats.total_gear_changes || 0, label: 'Gear Changes' },
      { value: formatDuration(lifetimeStats.total_time_at_high_rpm_seconds || 0), label: 'Time at High RPM' },

      // Performance Stats
      { value: `${(lifetimeStats.highest_engine_temp || 0).toFixed(0)}°C`, label: 'Highest Engine Temp' },
      { value: `${(lifetimeStats.highest_turbo_pressure || 0).toFixed(2)} bar`, label: 'Highest Turbo' },
      { value: `${(lifetimeStats.highest_oil_temp || 0).toFixed(0)}°C`, label: 'Highest Oil Temp' },
      { value: `${(lifetimeStats.total_fuel_consumed || 0).toFixed(1)} L`, label: 'Fuel Consumed' },
      { value: formatDuration(lifetimeStats.time_wot_seconds || 0), label: 'Time at WOT' },
      { value: lifetimeStats.hard_braking_events || 0, label: 'Hard Braking' },

      // Light Usage
      { value: formatDuration(lifetimeStats.time_highbeam_seconds || 0), label: 'High Beam Time' },
      { value: lifetimeStats.signal_activations || 0, label: 'Signal Uses' },
      { value: lifetimeStats.handbrake_pulls || 0, label: 'Handbrake Pulls' },
      { value: lifetimeStats.abs_activations || 0, label: 'ABS Activations' },

      // App Stats
      { value: formatDuration(lifetimeStats.total_app_time_seconds || 0), label: 'Lifetime App Time' }
    ];

    container.innerHTML = statCards.map(card => `
      <div class="stat-card">
        <div class="stat-card-value">${card.value}</div>
        <div class="stat-card-label">${card.label}</div>
      </div>
    `).join('');
  };

  /**
   * Populate log file selector
   */
  const populateLogSelector = () => {
    const select = document.getElementById('session-log-select');
    if (!select) return;

    select.innerHTML = '<option value="">Select a session log...</option>';
    
    sessionLogFiles.forEach(filename => {
      const option = document.createElement('option');
      option.value = filename;
      option.textContent = formatLogFilename(filename);
      select.appendChild(option);
    });
  };

  /**
   * Format log filename for display
   */
  const formatLogFilename = (filename) => {
    // Extract date from session_YYYY-MM-DD_HH-MM-SS.log
    const match = filename.match(/session_(\d{4}-\d{2}-\d{2})_(\d{2}-\d{2}-\d{2})\.log/);
    if (match) {
      return `${match[1]} at ${match[2].replace(/-/g, ':')}`;
    }
    return filename;
  };

  /**
   * Load and display session log
   */
  const loadSessionLog = async (filename) => {
    const viewer = document.getElementById('session-log-viewer');
    if (!viewer) return;

    if (!filename) {
      viewer.innerHTML = '<div class="session-log-empty">Select a session log to view</div>';
      return;
    }

    try {
      const content = await window.TauriBridge.readSessionLog(filename);
      renderLogContent(content);
    } catch (error) {
      console.error('[SessionStats] Failed to load log:', error);
      viewer.innerHTML = `<div class="session-log-empty">Failed to load log: ${window.Utils.sanitizeErrorMessage(error)}</div>`;
    }
  };

  /**
   * Render log content with color coding
   */
  const renderLogContent = (content) => {
    const viewer = document.getElementById('session-log-viewer');
    if (!viewer) return;

    const lines = content.split('\n').filter(line => line.trim());
    
    if (lines.length === 0) {
      viewer.innerHTML = '<div class="session-log-empty">Log file is empty</div>';
      return;
    }

    viewer.innerHTML = lines.map(line => {
      const entry = parseLogEntry(line);
      const typeClass = getLogTypeClass(entry.type);
      return `
        <div class="session-log-entry ${typeClass}">
          <span class="session-log-timestamp">${entry.timestamp}</span>
          <span class="session-log-type">[${entry.type}]</span>
          <span class="session-log-message">${window.Utils.escapeHtml(entry.message)}</span>
        </div>
      `;
    }).join('');

    // Scroll to bottom
    viewer.scrollTop = viewer.scrollHeight;
  };

  /**
   * Parse a log entry line
   */
  const parseLogEntry = (line) => {
    // Format: [HH:MM:SS] [EVENT_TYPE] data_as_json
    const match = line.match(/^\[(\d{2}:\d{2}:\d{2})\]\s+\[(\w+)\]\s+(.*)$/);
    if (match) {
      return {
        timestamp: match[1],
        type: match[2],
        message: match[3]
      };
    }
    return {
      timestamp: '--:--:--',
      type: 'GENERAL',
      message: line
    };
  };

  /**
   * Get CSS class for log entry type
   */
  const getLogTypeClass = (type) => {
    const typeUpper = type.toUpperCase();
    
    if (typeUpper === 'CRASH') return 'crash';
    if (typeUpper === 'TOP_SPEED' || typeUpper === 'HIGHEST_RPM') return 'record';
    if (typeUpper === 'SESSION_START' || typeUpper === 'SESSION_END') return 'general';
    
    return 'general';
  };

  /**
   * Export session log
   */
  const exportSessionLog = async () => {
    const select = document.getElementById('session-log-select');
    if (!select || !select.value) {
      window.Utils.showNotification('Please select a session log to export', 'warning');
      return;
    }

    try {
      const content = await window.TauriBridge.readSessionLog(select.value);
      downloadFile(content, select.value, 'text/plain');
      window.Utils.showNotification(`Exported ${select.value}`, 'success');
    } catch (error) {
      console.error('[SessionStats] Failed to export log:', error);
      window.Utils.showNotification('Failed to export log: ' + window.Utils.sanitizeErrorMessage(error), 'error');
    }
  };

  /**
   * Download file helper
   */
  const downloadFile = (content, filename, mimeType) => {
    const blob = new Blob([content], { type: mimeType });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  };

  /**
   * Reset lifetime stats with confirmation
   */
  const resetLifetimeStats = async () => {
    const confirmed = await showResetConfirmation();
    if (!confirmed) return;

    try {
      await window.TauriBridge.resetLifetimeStats();
      window.Utils.showNotification('Lifetime stats reset successfully', 'success');
      window.Utils.logToUI('success', 'Lifetime stats reset by user');
      await loadLifetimeStats();
    } catch (error) {
      console.error('[SessionStats] Failed to reset stats:', error);
      window.Utils.showNotification('Failed to reset stats: ' + window.Utils.sanitizeErrorMessage(error), 'error');
    }
  };

  /**
   * Show reset confirmation modal
   */
  const showResetConfirmation = () => {
    return new Promise((resolve) => {
      const modal = document.createElement('div');
      modal.className = 'modal-overlay';
      modal.innerHTML = `
        <div class="modal-content" style="max-width: 400px;">
          <div class="modal-header">
            <h3>Reset Lifetime Stats</h3>
          </div>
          <div class="modal-body">
            <p>Are you sure you want to reset all lifetime statistics?</p>
            <p style="color: var(--error-color); margin-top: 12px;">This action cannot be undone!</p>
          </div>
          <div class="modal-footer" style="display: flex; gap: 12px; justify-content: flex-end;">
            <button class="btn-secondary" id="reset-cancel-btn">Cancel</button>
            <button class="btn-primary" id="reset-confirm-btn" style="background: var(--error-color);">Reset All Stats</button>
          </div>
        </div>
      `;

      document.body.appendChild(modal);

      const cancelBtn = modal.querySelector('#reset-cancel-btn');
      const confirmBtn = modal.querySelector('#reset-confirm-btn');

      const closeModal = () => {
        modal.remove();
        resolve(false);
      };

      cancelBtn.addEventListener('click', closeModal);

      confirmBtn.addEventListener('click', () => {
        modal.remove();
        resolve(true);
      });

      // Close on overlay click
      modal.addEventListener('click', (e) => {
        if (e.target === modal) {
          closeModal();
        }
      });
    });
  };

  /**
   * Setup event listeners
   */
  const setupEventListeners = () => {
    // Log selector change
    const logSelect = document.getElementById('session-log-select');
    if (logSelect) {
      logSelect.addEventListener('change', (e) => {
        loadSessionLog(e.target.value);
      });
    }

    // Export button
    const exportBtn = document.getElementById('session-log-export');
    if (exportBtn) {
      exportBtn.addEventListener('click', exportSessionLog);
    }

    // Reset button
    const resetBtn = document.getElementById('session-reset-lifetime');
    if (resetBtn) {
      resetBtn.addEventListener('click', resetLifetimeStats);
    }
  };

  /**
   * Start session timer
   */
  const startSessionTimer = () => {
    // Update every second
    if (currentSessionInterval) {
      clearInterval(currentSessionInterval);
    }

    currentSessionInterval = setInterval(() => {
      updateSessionDuration();
    }, 1000);
  };

  /**
   * Start stats polling - poll backend for real-time stats updates
   */
  const startStatsPolling = () => {
    stopStatsPolling();

    // Session stats every 2.5s
    statsUpdateInterval = setInterval(async () => {
      await loadSessionStats();
    }, 2500);

    // Lifetime stats every 15s
    lifetimeStatsInterval = setInterval(async () => {
      await loadLifetimeStats();
    }, 15000);
  };

  /**
   * Stop stats polling
   */
  const stopStatsPolling = () => {
    if (statsUpdateInterval) {
      clearInterval(statsUpdateInterval);
      statsUpdateInterval = null;
    }
    if (lifetimeStatsInterval) {
      clearInterval(lifetimeStatsInterval);
      lifetimeStatsInterval = null;
    }
  };

  /**
   * Update session duration display
   */
  const updateSessionDuration = () => {
    const durationEl = document.getElementById('live-session-duration');
    if (!durationEl) return;

    const duration = calculateSessionDuration();
    durationEl.textContent = formatDuration(duration);
  };

  /**
   * Calculate session duration
   */
  const calculateSessionDuration = () => {
    if (!sessionStats || !sessionStats.session_start_time) {
      return 0;
    }

    const now = Math.floor(Date.now() / 1000);
    return now - sessionStats.session_start_time;
  };

  /**
   * Format duration in seconds to human readable string
   */
  const formatDuration = (seconds) => {
    const hrs = Math.floor(seconds / 3600);
    const mins = Math.floor((seconds % 3600) / 60);
    const secs = seconds % 60;

    if (hrs > 0) {
      return `${hrs}:${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
    }
    return `${mins}:${secs.toString().padStart(2, '0')}`;
  };

  /**
   * Format distance
   */
  const formatDistance = (km) => {
    if (km >= 1000) {
      return `${(km / 1000).toFixed(1)}k km`;
    }
    return `${km.toFixed(1)} km`;
  };

  /**
   * Refresh stats (called when panel is shown)
   */
  const refreshStats = async () => {
    await loadSessionStats();
    await loadLifetimeStats();
    await loadSessionLogFiles();
    // Restart polling if not already running
    startStatsPolling();
  };

  // Expose Session Stats module globally
  window.SessionStats = {
    initSessionStats,
    refreshStats,
    loadSessionStats,
    loadLifetimeStats,
    loadSessionLogFiles,
    exportSessionLog,
    resetLifetimeStats,
    stopStatsPolling
  };

  // Initialize on DOM ready
  document.addEventListener('DOMContentLoaded', () => {
    initSessionStats();
  });

})();
