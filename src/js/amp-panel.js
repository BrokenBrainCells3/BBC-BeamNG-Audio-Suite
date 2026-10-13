/**
 * Amp Panel - live view of the amplifier/subwoofer/enclosure telemetry
 * received from the BeamNG mod:
 *
 *   jbeam parts -> bbcAmpConfig -> ampSimGE -> ws://...:5001 / UDP 5001
 *     -> amp_state.rs -> "amp_state_updated" event + get_amp_state command
 *
 * Renders the live dynamic values (voltage, temps, clip, remote knob...) and
 * the static config cards (amp / sub driver / enclosure) exactly as the mod
 * sends them. Rows are built once and updated in place on every packet.
 */
(function() {
  'use strict';

  // [key, label, formatter] — fmt: v = raw value from the packet
  const LIVE_FIELDS = [
    ['supply_volts', 'Supply', v => fmtNum(v, 1) + ' V'],
    ['rpm', 'RPM', v => fmtNum(v, 0)],
    ['engine_on', 'Engine', fmtBool],
    ['remote_level', 'Remote Knob', v => fmtPct(v)],
    ['sub_level', 'Sub Demand', v => fmtPct(v)],
    ['clip', 'Clipping', v => fmtBoolWarn(v, true)],
    ['clip_db_over', 'Clip Over', v => (v > 0 ? '+' + fmtNum(v, 1) : fmtNum(v, 1)) + ' dB'],
    ['amp_temp_c', 'Amp Temp', v => fmtNum(v, 1) + ' °C'],
    ['thermal_protect', 'Thermal Protect', v => fmtBoolWarn(v, true)],
    ['coil_temp_c', 'Coil Temp', v => fmtNum(v, 1) + ' °C'],
    ['power_compression_db', 'Pwr Compression', v => fmtNum(v, 1) + ' dB'],
    ['breakin_pct', 'Break-in', v => fmtNum(v, 0) + '%'],
    ['leak_pct', 'Box Leak', v => fmtNum(v, 1) + '%'],
    ['fuse_blown', 'Fuse', v => fmtBoolWarn(v, true)],
  ];

  const AMP_FIELDS = [
    ['gain_db', 'Gain', v => '+' + fmtNum(v, 1) + ' dB'],
    ['preout_v', 'Pre-out', v => fmtNum(v, 1) + ' V'],
    ['bass_boost_db', 'Bass Boost', v => '+' + fmtNum(v, 1) + ' dB'],
    ['bass_boost_freq_hz', 'Boost Freq', v => fmtNum(v, 0) + ' Hz'],
    ['subsonic_hz', 'Subsonic', v => fmtNum(v, 0) + ' Hz'],
    ['subsonic_slope_db_oct', 'Subsonic Slope', v => fmtNum(v, 0) + ' dB/oct'],
    ['lpf_hz', 'LPF', v => fmtNum(v, 0) + ' Hz'],
    ['lpf_slope_db_oct', 'LPF Slope', v => fmtNum(v, 0) + ' dB/oct'],
    ['phase_deg', 'Phase', v => fmtNum(v, 0) + '°'],
    ['rms_power_w', 'RMS Power', v => fmtNum(v, 0) + ' W'],
    ['impedance_ohm', 'Load', v => fmtNum(v, 2) + ' Ω'],
    ['bridged', 'Bridged', fmtBool],
    ['channel_config', 'Channels', v => String(v)],
    ['amp_class', 'Class', v => String(v)],
    ['efficiency', 'Efficiency', v => fmtPct(v)],
    ['damping_factor', 'Damping Factor', v => fmtNum(v, 0)],
    ['fuse_a', 'Fuse Rating', v => fmtNum(v, 0) + ' A'],
  ];

  const SUB_FIELDS = [
    ['sub_count', 'Count', v => fmtNum(v, 0)],
    ['sub_size_in', 'Size', v => fmtNum(v, 0) + '"' ],
    ['sub_sensitivity_db', 'Sensitivity', v => fmtNum(v, 0) + ' dB'],
    ['sub_rms_w', 'RMS Handling', v => fmtNum(v, 0) + ' W'],
    ['sub_peak_w', 'Peak Handling', v => fmtNum(v, 0) + ' W'],
    ['sub_nominal_ohm', 'Nominal Imp.', v => fmtNum(v, 1) + ' Ω'],
    ['vc_config', 'Voice Coils', v => String(v)],
    ['vc_wiring', 'Wiring', v => String(v)],
    ['sd_cm2', 'Piston Area', v => fmtNum(v, 0) + ' cm²'],
    ['xmax_mm', 'Xmax', v => fmtNum(v, 0) + ' mm'],
    ['fs_hz', 'Fs', v => fmtNum(v, 1) + ' Hz'],
    ['qts', 'Qts', v => fmtNum(v, 2)],
    ['vas_l', 'Vas', v => fmtNum(v, 1) + ' L'],
    ['qes', 'Qes', v => fmtNum(v, 2)],
    ['qms', 'Qms', v => fmtNum(v, 1)],
    ['vc_inch', 'VC Size', v => fmtNum(v, 1) + '"' ],
    ['vc_wire', 'VC Wire', v => String(v).toUpperCase()],
    ['vc_former', 'VC Former', v => String(v)],
    ['bl_tm', 'BL', v => fmtNum(v, 1) + ' T·m'],
    ['magnet_oz', 'Magnet', v => fmtNum(v, 0) + ' oz'],
    ['cone_mat', 'Cone', v => String(v)],
    ['surround_mat', 'Surround', v => String(v)],
    ['spider_mat', 'Spider', v => String(v)],
    ['suspension_stiffness', 'Stiffness', v => fmtNum(v, 2)],
  ];

  const BOX_FIELDS = [
    ['enclosure_type', 'Type', v => String(v)],
    ['enclosure_l', 'Volume', v => fmtNum(v, 0) + ' L'],
    ['port_tune_hz', 'Port Tune', v => fmtNum(v, 0) + ' Hz'],
    ['port_area_cm2', 'Port Area', v => fmtNum(v, 0) + ' cm²'],
    ['port_len_cm', 'Port Length', v => fmtNum(v, 0) + ' cm'],
    ['port_shape', 'Port Shape', v => String(v)],
    ['enclosure_mat', 'Material', v => String(v)],
    ['enclosure_thick_mm', 'Thickness', v => fmtNum(v, 0) + ' mm'],
    ['bracing_pct', 'Bracing', v => fmtNum(v, 0) + '%'],
    ['polyfill_pct', 'Polyfill', v => fmtNum(v, 0) + '%'],
    ['sub_facing', 'Facing', v => String(v)],
    ['cabin_gain_db', 'Cabin Gain', v => '+' + fmtNum(v, 0) + ' dB'],
    ['leak_pct', 'Leaks', v => fmtNum(v, 1) + '%'],
  ];

  // ------------------------------------------------------------------

  // Override controls: moving a control sets an app-side override that beats
  // the game packet (sticky until cleared). Works even with no game running.
  const OVERRIDABLE = [
    // input stage + EQ
    { key: 'gain_db', label: 'Gain (dB)', type: 'slider', min: 0, max: 32, step: 0.5 },
    { key: 'preout_v', label: 'Pre-out (V)', type: 'slider', min: 0.5, max: 8, step: 0.1 },
    { key: 'bass_boost_db', label: 'Bass Boost (dB)', type: 'slider', min: 0, max: 18, step: 0.5 },
    { key: 'bass_boost_freq_hz', label: 'Boost Freq (Hz)', type: 'slider', min: 25, max: 80, step: 1 },
    { key: 'subsonic_hz', label: 'Subsonic (Hz)', type: 'slider', min: 10, max: 60, step: 1 },
    { key: 'lpf_hz', label: 'LPF (Hz)', type: 'slider', min: 30, max: 250, step: 1 },
    { key: 'phase_deg', label: 'Phase (deg)', type: 'slider', min: 0, max: 180, step: 15 },
    // amp
    { key: 'rms_power_w', label: 'Amp RMS (W)', type: 'slider', min: 100, max: 8000, step: 50 },
    { key: 'efficiency', label: 'Efficiency', type: 'slider', min: 0.4, max: 0.95, step: 0.01 },
    { key: 'damping_factor', label: 'Damping Factor', type: 'slider', min: 10, max: 1000, step: 10 },
    { key: 'impedance_ohm', label: 'Load (Ohm)', type: 'slider', min: 0.5, max: 8, step: 0.5 },
    { key: 'fuse_a', label: 'Fuse (A)', type: 'slider', min: 20, max: 300, step: 10 },
    { key: 'fuse_blown', label: 'Fuse Blown', type: 'toggle' },
    { key: 'thermal_protect', label: 'Thermal Protect', type: 'toggle' },
    { key: 'bridged', label: 'Bridged', type: 'toggle' },
    // driver
    { key: 'sub_count', label: 'Sub Count', type: 'slider', min: 1, max: 8, step: 1 },
    { key: 'sub_sensitivity_db', label: 'Sensitivity (dB)', type: 'slider', min: 78, max: 95, step: 1 },
    { key: 'sd_cm2', label: 'Piston (cm2)', type: 'slider', min: 150, max: 1300, step: 10 },
    { key: 'xmax_mm', label: 'Xmax (mm)', type: 'slider', min: 6, max: 32, step: 1 },
    { key: 'sub_rms_w', label: 'Sub RMS (W)', type: 'slider', min: 200, max: 3000, step: 50 },
    { key: 'sub_peak_w', label: 'Sub Peak (W)', type: 'slider', min: 300, max: 5000, step: 100 },
    { key: 'fs_hz', label: 'Fs (Hz)', type: 'slider', min: 15, max: 60, step: 1 },
    { key: 'vas_l', label: 'Vas (L)', type: 'slider', min: 5, max: 150, step: 1 },
    { key: 'qts', label: 'Qts', type: 'slider', min: 0.2, max: 0.9, step: 0.01 },
    { key: 'bl_tm', label: 'BL (T*m)', type: 'slider', min: 5, max: 40, step: 1 },
    { key: 'suspension_stiffness', label: 'Stiffness', type: 'slider', min: 0.7, max: 1.3, step: 0.01 },
    // enclosure
    { key: 'enclosure_type', label: 'Box Type', type: 'select', options: ['ported', 'sealed', 'bandpass', 'free-air', 'infinite-baffle'] },
    { key: 'enclosure_l', label: 'Box Volume (L)', type: 'slider', min: 10, max: 200, step: 1 },
    { key: 'port_tune_hz', label: 'Port Tune (Hz)', type: 'slider', min: 22, max: 50, step: 1 },
    { key: 'enclosure_mat', label: 'Box Material', type: 'select', options: ['mdf', 'birch', 'ply', 'steel', 'alu'] },
    { key: 'enclosure_thick_mm', label: 'Thickness (mm)', type: 'slider', min: 6, max: 40, step: 1 },
    { key: 'bracing_pct', label: 'Bracing (%)', type: 'slider', min: 0, max: 100, step: 1 },
    { key: 'polyfill_pct', label: 'Polyfill (%)', type: 'slider', min: 0, max: 100, step: 5 },
    { key: 'leak_pct', label: 'Leaks (%)', type: 'slider', min: 0, max: 60, step: 1 },
    { key: 'sub_facing', label: 'Facing', type: 'select', options: ['up', 'back', 'forward'] },
    { key: 'cabin_gain_db', label: 'Cabin Gain (dB)', type: 'slider', min: 0, max: 12, step: 0.5 },
    // live knobs
    { key: 'remote_level', label: 'Remote Knob', type: 'slider', min: 0, max: 1, step: 0.01 },
    { key: 'supply_volts', label: 'Supply (V)', type: 'slider', min: 8, max: 18, step: 0.1 },
  ];

  let overrides = {}; // key -> value (synced with backend)
  let overridesBuilt = false;
  let lastPacket = null; // latest amp_state payload for live slider following

  function buildOverrides() {
    const grid = document.getElementById('amp-ovr-grid');
    if (!grid || overridesBuilt) return;
    overridesBuilt = true;
    grid.innerHTML = '';
    for (const f of OVERRIDABLE) {
      const row = document.createElement('div');
      row.className = 'amp-ovr-row';
      row.id = 'amp-ovr-row-' + f.key;

      const label = document.createElement('span');
      label.className = 'amp-k';
      label.textContent = f.label;

      const ctrl = document.createElement('div');
      ctrl.className = 'amp-ovr-ctrl';

      let input;
      if (f.type === 'slider') {
        input = document.createElement('input');
        input.type = 'range';
        input.min = f.min; input.max = f.max; input.step = f.step;
        input.value = f.min;
        input.addEventListener('input', () => {
          const v = parseFloat(input.value);
          setOverride(f.key, v, row);
          const val = row.querySelector('.amp-ovr-val');
          if (val) val.textContent = formatOvr(f, v);
        });
      } else if (f.type === 'toggle') {
        input = document.createElement('input');
        input.type = 'checkbox';
        input.addEventListener('change', () => {
          setOverride(f.key, input.checked, row);
        });
      } else if (f.type === 'select') {
        input = document.createElement('select');
        for (const o of f.options) {
          const opt = document.createElement('option');
          opt.value = o; opt.textContent = o;
          input.appendChild(opt);
        }
        input.addEventListener('change', () => {
          setOverride(f.key, input.value, row);
        });
      }
      ctrl.appendChild(input);

      const val = document.createElement('span');
      val.className = 'amp-ovr-val';
      ctrl.appendChild(val);

      const clear = document.createElement('button');
      clear.className = 'amp-ovr-clear';
      clear.textContent = '✕';
      clear.title = 'Revert to live game value';
      clear.addEventListener('click', () => clearOverride(f.key, row, f));
      ctrl.appendChild(clear);

      row.appendChild(label);
      row.appendChild(ctrl);
      grid.appendChild(row);
    }

    const clearAll = document.getElementById('amp-ovr-clear');
    if (clearAll) {
      clearAll.addEventListener('click', async () => {
        try {
          await window.TauriBridge.clearAmpOverrides();
          overrides = {};
          for (const f of OVERRIDABLE) {
            const row = document.getElementById('amp-ovr-row-' + f.key);
            if (row) { row.classList.remove('amp-ovr-on'); updateOvrDisplay(f, row, null); }
          }
          window.Utils.showNotification('Amp overrides cleared', 'info');
        } catch (e) {
          console.error('[AmpPanel] clearAmpOverrides failed', e);
        }
      });
    }
  }

  function formatOvr(f, v) {
    if (v === null || v === undefined) return '';
    if (f.type === 'toggle') return v ? 'ON' : 'OFF';
    if (typeof v === 'number') return v.toFixed(f.step < 0.1 ? 2 : (f.step < 1 ? 1 : 0));
    return String(v);
  }

  async function setOverride(key, value, row) {
    overrides[key] = value;
    if (row) row.classList.add('amp-ovr-on');
    try {
      await window.TauriBridge.setAmpOverride(key, value);
    } catch (e) {
      console.error('[AmpPanel] setAmpOverride failed', e);
    }
  }

  async function clearOverride(key, row, f) {
    delete overrides[key];
    if (row) row.classList.remove('amp-ovr-on');
    try {
      await window.TauriBridge.clearAmpOverride(key);
    } catch (e) { console.error(e); }
    updateOvrDisplay(f, row, null);
  }

  function updateOvrDisplay(f, row, packetValue) {
    const val = row && row.querySelector('.amp-ovr-val');
    const ctrl = row && row.querySelector('.amp-ovr-ctrl > :first-child');
    if (!row) return;
    const isOn = overrides.hasOwnProperty(f.key);
    if (isOn) {
      if (val) val.textContent = formatOvr(f, overrides[f.key]);
    } else if (packetValue !== undefined && packetValue !== null) {
      // follow the live game value while not overridden
      if (f.type === 'slider' && ctrl) ctrl.value = packetValue;
      if (f.type === 'toggle' && ctrl) ctrl.checked = !!packetValue;
      if (f.type === 'select' && ctrl && packetValue !== '') ctrl.value = packetValue;
      if (val) val.textContent = formatOvr(f, packetValue);
    }
  }

  function syncOverridesFromPacket(d) {
    if (!d || !overridesBuilt) return;
    for (const f of OVERRIDABLE) {
      const row = document.getElementById('amp-ovr-row-' + f.key);
      if (!row) continue;
      if (!overrides.hasOwnProperty(f.key)) {
        updateOvrDisplay(f, row, d[f.key]);
      }
    }
  }

  async function loadOverrides() {
    try {
      const ov = await window.TauriBridge.getAmpOverrides();
      overrides = ov || {};
      for (const f of OVERRIDABLE) {
        const row = document.getElementById('amp-ovr-row-' + f.key);
        if (!row) continue;
        if (overrides.hasOwnProperty(f.key)) {
          row.classList.add('amp-ovr-on');
          const ctrl = row.querySelector('.amp-ovr-ctrl > :first-child');
          if (f.type === 'slider' && ctrl) ctrl.value = overrides[f.key];
          if (f.type === 'toggle' && ctrl) ctrl.checked = !!overrides[f.key];
          if (f.type === 'select' && ctrl) ctrl.value = overrides[f.key];
          updateOvrDisplay(f, row, null);
        }
      }
    } catch (e) { /* backend not ready */ }
  }

  function fmtNum(v, decimals) {
    const n = Number(v);
    if (!isFinite(n)) return '—';
    return n.toFixed(decimals);
  }
  function fmtPct(v) {
    const n = Number(v);
    if (!isFinite(n)) return '—';
    return Math.round(n * 100) + '%';
  }
  function fmtBool(v) {
    return v ? 'Yes' : 'No';
  }
  // Boolean that's interesting when true (clip, fuse blown, protect...)
  function fmtBoolWarn(v, warnWhenTrue) {
    if (v) return warnWhenTrue ? 'YES' : 'Yes';
    return 'No';
  }

  function buildCardGrid(gridId, fields) {
    const grid = document.getElementById(gridId);
    if (!grid) return;
    grid.innerHTML = '';
    for (const [key, label] of fields) {
      const row = document.createElement('div');
      row.className = 'amp-row';
      const k = document.createElement('span');
      k.className = 'amp-k';
      k.textContent = label;
      const val = document.createElement('span');
      val.className = 'amp-v';
      val.id = 'amp-v-' + key;
      val.textContent = '—';
      row.appendChild(k);
      row.appendChild(val);
      grid.appendChild(row);
    }
  }

  function applyFields(d, fields) {
    if (!d) return;
    for (const [key, label, fmt] of fields) {
      const el = document.getElementById('amp-v-' + key);
      if (!el) continue;
      const has = Object.prototype.hasOwnProperty.call(d, key) && d[key] !== null;
      el.textContent = has ? fmt(d[key]) : '—';
      // warn styling for hot/bad states
      const warn = key === 'clip' || key === 'fuse_blown' || key === 'thermal_protect';
      el.classList.toggle('amp-warn', warn && d[key] === true);
      if (key === 'amp_temp_c' || key === 'coil_temp_c') {
        el.classList.toggle('amp-warn', Number(d[key]) > 80);
      }
    }
  }

  function render(d) {
    lastPacket = d;
    applyFields(d, LIVE_FIELDS);
    applyFields(d, AMP_FIELDS);
    applyFields(d, SUB_FIELDS);
    applyFields(d, BOX_FIELDS);
    syncOverridesFromPacket(d);
  }

  function renderStatus(s, overrideCount) {
    const dot = document.getElementById('amp-status-dot');
    const text = document.getElementById('amp-status-text');
    if (!dot || !text || !s) return;
    let msg;
    if (s.receiving) {
      dot.className = 'amp-status-dot amp-ok';
      msg = 'Receiving — ' + s.packets + ' packets, last ' +
        (s.last_packet_age_ms < 1000 ? 'just now' : Math.round(s.last_packet_age_ms / 1000) + 's ago');
    } else if (s.packets > 0) {
      dot.className = 'amp-status-dot amp-stale';
      msg = 'Stale — no packets for ' + Math.round(s.last_packet_age_ms / 1000) + 's (' + s.packets + ' total)';
    } else {
      dot.className = 'amp-status-dot amp-off';
      msg = 'No amp_state yet — spawn a vehicle with the ampSim controller in game';
    }
    if (overrideCount > 0) {
      msg += '  |  ' + overrideCount + ' APP OVERRIDE' + (overrideCount > 1 ? 'S' : '') +
        ' ACTIVE (they beat game values — Clear All to revert)';
    }
    text.textContent = msg;
  }

  async function init() {
    if (!window.TauriBridge) {
      console.warn('[AmpPanel] TauriBridge not ready');
      return;
    }

    buildCardGrid('amp-live-grid', LIVE_FIELDS);
    buildCardGrid('amp-card-amp', AMP_FIELDS);
    buildCardGrid('amp-card-sub', SUB_FIELDS);
    buildCardGrid('amp-card-box', BOX_FIELDS);
    buildOverrides();
    loadOverrides();

    try {
      await window.TauriBridge.onAmpState(render);
    } catch (e) {
      console.error('[AmpPanel] onAmpState failed', e);
    }

    // Status line refresh (packets counter + age live here, not in the packet)
    const refreshStatus = async () => {
      try {
        const s = await window.TauriBridge.getAmpStateStatus();
        let overrideCount = Object.keys(overrides).length;
        try {
          const ov = await window.TauriBridge.getAmpOverrides();
          overrides = ov || {};
          overrideCount = Object.keys(overrides).length;
          // sync row highlight + displays for externally-cleared overrides
          for (const f of OVERRIDABLE) {
            const row = document.getElementById('amp-ovr-row-' + f.key);
            if (!row) continue;
            const isOn = overrides.hasOwnProperty(f.key);
            row.classList.toggle('amp-ovr-on', isOn);
            if (!isOn) updateOvrDisplay(f, row, lastPacket ? lastPacket[f.key] : undefined);
            else updateOvrDisplay(f, row, null);
          }
        } catch (e) { /* ignore */ }
        renderStatus(s, overrideCount);
      } catch (e) { /* backend not ready yet */ }
    };
    refreshStatus();
    setInterval(refreshStatus, 2000);

    // Initial fill
    try {
      render(await window.TauriBridge.getAmpState());
    } catch (e) { /* ignore */ }

    window.Utils.logToUI('info', 'Amp panel initialized');
  }

  // Expose module
  window.AmpPanel = { init, render, renderStatus };

  document.addEventListener('DOMContentLoaded', () => setTimeout(init, 150));
})();
