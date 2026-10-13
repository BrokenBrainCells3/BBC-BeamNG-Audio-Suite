/**
 * 3D Audio Module - 3D audio and spatial monitoring functionality
 * Uses window.TauriBridge for all Tauri API calls
 * Uses window.Utils for utility functions
 */

(function () {
  'use strict';

  // 3D Audio State
  let is3dAudioEnabled = false;
  let is3dAudioUdpEnabled = false;
  let speakerMutes = {
    fl_muted: false,
    fr_muted: false,
    rl_muted: false,
    rr_muted: false,
    sub_l_muted: false,
    sub_r_muted: false
  };
  let speakerLevels = {
    frontLeft: 0.25,
    frontRight: 0.25,
    rearLeft: 0.25,
    rearRight: 0.25,
    subwoofer: 0.5
  };
  let beamngTelemetry = {
    speed: 0,
    rpm: 0,
    throttle: 0
  };
  let telemetryUpdateInterval = null;

  // Waveform history for band displays
  const waveformHistory = {
    subwoofer: new Array(80).fill(0),
    bass: new Array(80).fill(0),
    mids: new Array(80).fill(0),
    treble: new Array(80).fill(0),
  };

  // Canvas contexts
  const contexts = {};
  let spatialPlotCtx = null;
  let isUdpMode = false;  // Track UDP mode globally

  // Cached DOM elements
  const indicatorEls = { fl: null, fr: null, rl: null, rr: null, sub: null };
  const canvasEls = {};

  // Colors for each band
  const bandColors = {
    subwoofer: "#ff6b6b",
    bass: "#4ecdc4",
    mids: "#f9d423",
    treble: "#a8edea",
  };

  /**
   * Initialize canvas for band waveform display
   */
  const initBandCanvas = (canvasId) => {
    const canvas = document.getElementById(canvasId);
    if (!canvas) return;

    canvas.width = canvas.offsetWidth;
    canvas.height = canvas.offsetHeight;

    const ctx = canvas.getContext("2d");
    contexts[canvasId.replace("-canvas", "")] = ctx;
  };

  /**
   * Initialize all band canvases
   */
  const initializeBandCanvases = () => {
    const canvasIds = ['subwoofer', 'bass', 'mids', 'treble'];
    canvasIds.forEach(id => initBandCanvas(`${id}-canvas`));
  };

  /**
   * Initialize spatial monitor canvas
   */
  const initializeSpatialCanvas = () => {
    initBandCanvas('spatial-canvas');
    initSpatialPlotCanvas();
  };

  /**
   * Initialize spatial plot canvas
   */
  const initSpatialPlotCanvas = () => {
    const canvas = document.getElementById('spatial-plot-canvas');
    if (!canvas) return;

    canvas.width = canvas.offsetWidth;
    canvas.height = canvas.offsetHeight;
    spatialPlotCtx = canvas.getContext('2d');
  };

  /**
   * Draw spatial position plot
   */
  const drawSpatialPlot = (data) => {
    if (!spatialPlotCtx) return;

    const canvas = document.getElementById('spatial-plot-canvas');
    if (!canvas) return;

    const ctx = spatialPlotCtx;
    const width = canvas.width;
    const height = canvas.height;

    // Clear canvas
    ctx.clearRect(0, 0, width, height);

    const vehPos = [data.veh_pos_x || 0, data.veh_pos_y || 0, data.veh_pos_z || 0];
    const camPos = [data.cam_global_x || 0, data.cam_global_y || 0, data.cam_global_z || 0];
    const camForward = [
      data.cam_global_forward_x || 0,
      data.cam_global_forward_y || 0,
      data.cam_global_forward_z || 0
    ];

    // Use isUdpMode set by handleBeamngCameraData
    const isUdpMode = window.Audio3D.isUdpMode || false;
    document.getElementById('plot-mode').textContent = isUdpMode ? 'UDP' : 'Manual';
    document.getElementById('plot-mode').className = 'stat-value ' + (isUdpMode ? 'mode-udp' : 'mode-manual');

    // Get speaker positions - UDP data is GLOBAL, manual mode is relative to vehicle
    const getSpeakerPos = (speakerData, defaultOffset) => {
      if (isUdpMode && speakerData && Array.isArray(speakerData) && speakerData.length === 3) {
        // UDP mode: speaker positions are already GLOBAL
        return [speakerData[0], speakerData[1], speakerData[2]];
      }
      // Manual mode: rotate offset by vehicle quaternion, then add to vehicle position
      const vehQuat = [
        data.veh_quat_x || 0,
        data.veh_quat_y || 0,
        data.veh_quat_z || 0,
        data.veh_quat_w || 0
      ];
      const rotated = rotateVectorByQuat(defaultOffset, vehQuat);
      return [
        vehPos[0] + rotated[0],
        vehPos[1] + rotated[1],
        vehPos[2] + rotated[2]
      ];
    };

    // Quaternion rotation helper (matches Rust implementation)
    function rotateVectorByQuat(v, q) {
      const [x, y, z] = v;
      const [qx, qy, qz, qw] = q;

      // First product: q * (0, v)
      const ix = qw * x + qy * z - qz * y;
      const iy = qw * y + qz * x - qx * z;
      const iz = qw * z + qx * y - qy * x;
      const iw = -qx * x - qy * y - qz * z;

      // Second product: (ix, iy, iz, iw) * q_conjugate
      // q_conjugate = (qw, -qx, -qy, -qz)
      return [
        ix * qw + iw * (-qx) + iy * qz - iz * qy,
        iy * qw + iw * (-qy) + iz * qx - ix * qz,
        iz * qw + iw * (-qz) + ix * qy - iy * qx
      ];
    }

    const speakerPositions = {
      fl: getSpeakerPos(data.speaker_driver_main, [-0.9, -2.0, 0.5]),
      fr: getSpeakerPos(data.speaker_passenger_main, [0.9, -2.0, 0.5]),
      rl: getSpeakerPos(data.speaker_driver_rear, [-0.9, 2.0, 0.5]),
      rr: getSpeakerPos(data.speaker_passenger_rear, [0.9, 2.0, 0.5]),
      sub_l: getSpeakerPos(data.speaker_driver_sub, [-0.3, 1.5, 0.3]),
      sub_r: getSpeakerPos(data.speaker_passenger_sub, [0.3, 1.5, 0.3]),
      twt_l: getSpeakerPos(data.speaker_driver_twt, [-0.9, -2.0, 0.6]),
      twt_r: getSpeakerPos(data.speaker_passenger_twt, [0.9, -2.0, 0.6]),
    };

    // A speaker whose UDP node wasn't found arrives as (0,0,0); skip it so the
    // plot only shows speakers that actually exist. Also prevents zoom breaks.
    const isValidSpeaker = (pos) => {
      const mag = Math.sqrt(pos[0] ** 2 + pos[1] ** 2 + pos[2] ** 2);
      return mag > 0.5;
    };

    // Which UDP validity bit maps to each plot slot. A speaker only shows if
    // its node actually exists on the vehicle.
    const speakerValidMap = {
      fl: 1,
      rl: 2,
      sub_l: 8,
      fr: 16,
      rr: 32,
      sub_r: 128,
      twt_l: 256,
      twt_r: 512
    };
    const validFlags = data.speaker_valid_flags;
    const isSpeakerInstalled = (key) => {
      if (typeof validFlags === 'number') {
        return (validFlags & (speakerValidMap[key] || 0)) !== 0;
      }
      return isValidSpeaker(speakerPositions[key]);
    };

    // Calculate bounds for auto-scaling (exclude missing/invalid speakers)
    const allPoints = [vehPos, camPos];
    for (const key of ['fl', 'fr', 'rl', 'rr', 'sub_l', 'sub_r', 'twt_l', 'twt_r']) {
      if (isSpeakerInstalled(key)) allPoints.push(speakerPositions[key]);
    }

    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (const pt of allPoints) {
      minX = Math.min(minX, pt[0]);
      maxX = Math.max(maxX, pt[0]);
      minY = Math.min(minY, pt[1]);
      maxY = Math.max(maxY, pt[1]);
    }

    // Add padding and ensure minimum range
    const padding = 1.0;
    minX -= padding; maxX += padding;
    minY -= padding; maxY += padding;

    let rangeX = maxX - minX;
    let rangeY = maxY - minY;
    const minRange = 3.0;
    if (rangeX < minRange) {
      const center = (minX + maxX) / 2;
      minX = center - minRange / 2;
      maxX = center + minRange / 2;
      rangeX = minRange;
    }
    if (rangeY < minRange) {
      const center = (minY + maxY) / 2;
      minY = center - minRange / 2;
      maxY = center + minRange / 2;
      rangeY = minRange;
    }

    // Coordinate transform (Y is up in 3D, but down in canvas)
    const toCanvas = (x, y) => [
      (x - minX) / rangeX * width,
      height - (y - minY) / rangeY * height
    ];

    // Draw grid
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.1)';
    ctx.lineWidth = 1;
    const gridSize = 1.0;
    for (let x = Math.floor(minX); x <= maxX; x += gridSize) {
      const [cx1, cy1] = toCanvas(x, minY);
      const [cx2, cy2] = toCanvas(x, maxY);
      ctx.beginPath();
      ctx.moveTo(cx1, cy1);
      ctx.lineTo(cx2, cy2);
      ctx.stroke();
    }
    for (let y = Math.floor(minY); y <= maxY; y += gridSize) {
      const [cx1, cy1] = toCanvas(minX, y);
      const [cx2, cy2] = toCanvas(maxX, y);
      ctx.beginPath();
      ctx.moveTo(cx1, cy1);
      ctx.lineTo(cx2, cy2);
      ctx.stroke();
    }

    // Draw vehicle outline (rectangle)
    const vehWidth = 1.8;
    const vehLength = 4.5;
    const [vehCx, vehCy] = toCanvas(vehPos[0], vehPos[1]);
    ctx.strokeStyle = 'rgba(78, 205, 196, 0.5)';
    ctx.lineWidth = 2;
    ctx.strokeRect(vehCx - vehWidth / 2, vehCy - vehLength / 2, vehWidth, vehLength);

    // Draw vehicle center point
    ctx.fillStyle = '#4ecdc4';
    ctx.beginPath();
    ctx.arc(vehCx, vehCy, 6, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = '#fff';
    ctx.font = '10px Arial';
    ctx.textAlign = 'center';
    ctx.fillText('VEH', vehCx, vehCy - 10);

    // Draw speakers
    const drawSpeaker = (pos, label, color) => {
      const [cx, cy] = toCanvas(pos[0], pos[1]);
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(cx, cy, 8, 0, Math.PI * 2);
      ctx.fill();
      ctx.strokeStyle = '#fff';
      ctx.lineWidth = 2;
      ctx.stroke();
      ctx.fillStyle = '#000';
      ctx.font = 'bold 9px Arial';
      ctx.textAlign = 'center';
      ctx.textBaseline = 'middle';
      ctx.fillText(label, cx, cy);

      // Draw distance label
      const dist = Math.sqrt(
        Math.pow(pos[0] - camPos[0], 2) +
        Math.pow(pos[1] - camPos[1], 2) +
        Math.pow(pos[2] - camPos[2], 2)
      );
      ctx.fillStyle = 'rgba(255, 255, 255, 0.7)';
      ctx.font = '10px Arial';
      ctx.fillText(dist.toFixed(2) + 'm', cx, cy - 12);
    };

    if (isSpeakerInstalled('fl')) drawSpeaker(speakerPositions.fl, 'FL', '#f9d423');
    if (isSpeakerInstalled('fr')) drawSpeaker(speakerPositions.fr, 'FR', '#f9d423');
    if (isSpeakerInstalled('rl')) drawSpeaker(speakerPositions.rl, 'RL', '#f9d423');
    if (isSpeakerInstalled('rr')) drawSpeaker(speakerPositions.rr, 'RR', '#f9d423');

    // Only draw subwoofers if they have valid positions (not at origin)
    if (isSpeakerInstalled('sub_l')) {
      drawSpeaker(speakerPositions.sub_l, 'SL', '#a8edea');
    }
    if (isSpeakerInstalled('sub_r')) {
      drawSpeaker(speakerPositions.sub_r, 'SR', '#a8edea');
    }

    // Optional tweeters (only drawn if their nodes exist)
    if (isSpeakerInstalled('twt_l')) {
      drawSpeaker(speakerPositions.twt_l, 'TL', '#ffd1dc');
    }
    if (isSpeakerInstalled('twt_r')) {
      drawSpeaker(speakerPositions.twt_r, 'TR', '#ffd1dc');
    }

    // Draw camera position
    const [camCx, camCy] = toCanvas(camPos[0], camPos[1]);
    ctx.fillStyle = '#ff6b6b';
    ctx.beginPath();
    ctx.arc(camCx, camCy, 10, 0, Math.PI * 2);
    ctx.fill();
    ctx.strokeStyle = '#fff';
    ctx.lineWidth = 2;
    ctx.stroke();
    ctx.fillStyle = '#fff';
    ctx.font = 'bold 11px Arial';
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText('CAM', camCx, camCy);

    // Draw camera direction arrow
    const camDirLength = Math.max(rangeX, rangeY) * 0.3;
    const [arrowEndX, arrowEndY] = toCanvas(
      camPos[0] + camForward[0] * camDirLength,
      camPos[1] + camForward[1] * camDirLength
    );

    ctx.strokeStyle = '#ff6b6b';
    ctx.lineWidth = 3;
    ctx.setLineDash([5, 3]);
    ctx.beginPath();
    ctx.moveTo(camCx, camCy);
    ctx.lineTo(arrowEndX, arrowEndY);
    ctx.stroke();
    ctx.setLineDash([]);

    // Arrow head
    const angle = Math.atan2(arrowEndY - camCy, arrowEndX - camCx);
    const arrowHeadLen = 10;
    ctx.beginPath();
    ctx.moveTo(arrowEndX, arrowEndY);
    ctx.lineTo(
      arrowEndX - arrowHeadLen * Math.cos(angle - Math.PI / 6),
      arrowEndY - arrowHeadLen * Math.sin(angle - Math.PI / 6)
    );
    ctx.lineTo(
      arrowEndX - arrowHeadLen * Math.cos(angle + Math.PI / 6),
      arrowEndY - arrowHeadLen * Math.sin(angle + Math.PI / 6)
    );
    ctx.closePath();
    ctx.fillStyle = '#ff6b6b';
    ctx.fill();

    // Update stats
    const camVehDist = Math.sqrt(
      Math.pow(camPos[0] - vehPos[0], 2) +
      Math.pow(camPos[1] - vehPos[1], 2) +
      Math.pow(camPos[2] - vehPos[2], 2)
    );
    document.getElementById('plot-cam-veh-dist').textContent = camVehDist.toFixed(2) + ' m';

    // Calculate camera heading (yaw) from forward vector
    const camHeading = Math.atan2(camForward[1], camForward[0]) * 180 / Math.PI;
    const normalizedHeading = ((camHeading % 360) + 360) % 360;
    document.getElementById('plot-cam-heading').textContent = normalizedHeading.toFixed(0) + '°';
  };

  /**
   * Draw waveform for a frequency band
   */
  const drawBandWaveform = (bandName, value) => {
    const ctx = contexts[bandName];
    const canvasId = `${bandName}-canvas`;
    const valueId = `${bandName}-value`;

    if (!canvasEls[canvasId]) canvasEls[canvasId] = document.getElementById(canvasId);
    if (!canvasEls[valueId]) canvasEls[valueId] = document.getElementById(valueId);

    const canvas = canvasEls[canvasId];

    if (!ctx || !canvas) return;

    const width = canvas.width;
    const height = canvas.height;

    waveformHistory[bandName].shift();
    waveformHistory[bandName].push(value);

    ctx.fillStyle = "rgba(0, 0, 0, 0.3)";
    ctx.fillRect(0, 0, width, height);

    const centerY = height / 2;

    ctx.strokeStyle = "rgba(128, 128, 128, 0.3)";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, centerY);
    ctx.lineTo(width, centerY);
    ctx.stroke();

    ctx.strokeStyle = bandColors[bandName];
    ctx.lineWidth = 2;
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    ctx.beginPath();

    for (let i = 0; i < waveformHistory[bandName].length; i++) {
      const x = (i / (waveformHistory[bandName].length - 1)) * width;
      const val = waveformHistory[bandName][i];
      const y = centerY - (val * (height / 2));

      if (i === 0) {
        ctx.moveTo(x, y);
      } else {
        ctx.lineTo(x, y);
      }
    }

    ctx.stroke();

    const valueEl = canvasEls[`${bandName}-value`];
    if (valueEl) {
      valueEl.textContent = value.toFixed(2);
    }
  };

  /**
   * Toggle 3D audio UDP mode
   */
  const toggle3dAudioUdp = async () => {
    const toggle = document.getElementById('3d-audio-udp-toggle');
    const visualizer = document.getElementById('audio-3d-visualizer');

    if (!toggle || !visualizer) return;

    is3dAudioUdpEnabled = toggle.checked;

    try {
      await window.TauriBridge.set3dAudioUdpEnabled(is3dAudioUdpEnabled);

      if (is3dAudioUdpEnabled) {
        visualizer.style.display = 'block';
        visualizer.classList.add('active');
        window.Utils.showNotification('3D Audio enabled (UDP Mode) - Using real-time speaker positions from BeamNG', 'success');
        startTelemetryUpdates();
      } else {
        window.Utils.showNotification('3D Audio (UDP Mode) disabled', 'info');
        visualizer.style.display = 'none';
        visualizer.classList.remove('active');
        stopTelemetryUpdates();
        updateSpeakerIndicators({ front_left: 0.25, front_right: 0.25, rear_left: 0.25, rear_right: 0.25, subwoofer: 0.5 });
      }
    } catch (error) {
      console.error('Failed to toggle 3D audio UDP:', error);
      window.Utils.logToUI('error', 'Failed to toggle 3D audio UDP');
      window.Utils.showNotification('Failed to toggle 3D audio UDP: ' + window.Utils.sanitizeErrorMessage(error), 'error');
      toggle.checked = !is3dAudioUdpEnabled;
      is3dAudioUdpEnabled = toggle.checked;
    }
  };

  /**
   * Update speaker level indicators
   */
  const updateSpeakerIndicators = (data) => {
    if (!indicatorEls.fl) indicatorEls.fl = document.getElementById('fl-indicator');
    if (!indicatorEls.fr) indicatorEls.fr = document.getElementById('fr-indicator');
    if (!indicatorEls.rl) indicatorEls.rl = document.getElementById('rl-indicator');
    if (!indicatorEls.rr) indicatorEls.rr = document.getElementById('rr-indicator');
    if (!indicatorEls.sub) indicatorEls.sub = document.getElementById('sub-indicator');

    if (indicatorEls.fl) indicatorEls.fl.style.height = `${(data.front_left || 0) * 100}%`;
    if (indicatorEls.fr) indicatorEls.fr.style.height = `${(data.front_right || 0) * 100}%`;
    if (indicatorEls.rl) indicatorEls.rl.style.height = `${(data.rear_left || 0) * 100}%`;
    if (indicatorEls.rr) indicatorEls.rr.style.height = `${(data.rear_right || 0) * 100}%`;
    if (indicatorEls.sub) indicatorEls.sub.style.height = `${(data.subwoofer || 0) * 100}%`;

    speakerLevels = {
      frontLeft: data.front_left || 0.25,
      frontRight: data.front_right || 0.25,
      rearLeft: data.rear_left || 0.25,
      rearRight: data.rear_right || 0.25,
      subwoofer: data.subwoofer || 0.5
    };
  };

  /**
   * Start telemetry updates
   */
  const startTelemetryUpdates = () => {
    if (telemetryUpdateInterval) return;

    if (window.UI && window.UI.startCameraPolling) {
      window.UI.startCameraPolling();
    }

    telemetryUpdateInterval = setInterval(async () => {
      if (!is3dAudioEnabled && !is3dAudioUdpEnabled) {
        stopTelemetryUpdates();
        return;
      }
      try {
        const state = await window.TauriBridge.get3dAudioState();
        beamngTelemetry = {
          speed: state.vehicle_speed || 0,
          rpm: state.engine_rpm || 0,
          throttle: state.throttle || 0
        };

        updateSpeakerIndicators({
          front_left: state.front_left,
          front_right: state.front_right,
          rear_left: state.rear_left,
          rear_right: state.rear_right,
          subwoofer: (state.subwoofer_left + state.subwoofer_right) / 2
        });
      } catch (error) {
        window.Utils.throttleLog('3d_audio_state', console.error, ['Failed to get 3D audio state:', error], 5000, 8000);
      }
    }, 500); // was 250
  };

  /**
   * Stop telemetry updates
   */
  const stopTelemetryUpdates = () => {
    if (telemetryUpdateInterval) {
      clearInterval(telemetryUpdateInterval);
      telemetryUpdateInterval = null;
    }
    if (window.UI && window.UI.stopCameraPolling) {
      window.UI.stopCameraPolling();
    }
  };

  /**
   * Handle BeamNG telemetry events
   */
  const handleBeamngTelemetry = (event) => {
    if (!is3dAudioEnabled) return;

    const data = event.payload;
    beamngTelemetry = {
      speed: data.speed || 0,
      rpm: data.rpm || 0,
      throttle: data.throttle || 0
    };

    updateSpeakerIndicators({
      front_left: data.front_left,
      front_right: data.front_right,
      rear_left: data.rear_left,
      rear_right: data.rear_right,
      subwoofer: data.subwoofer
    });
  };

  /**
   * Handle BeamNG camera data
   */
  const handleBeamngCameraData = (data) => {
    window.__beamngCameraData = data;

    // Update speaker LOS (line-of-sight) indicators from blocked flags.
    // Bit layout matches speaker_valid_flags in the UDP protocol:
    //   FL = driverspk (bit 0), RL = driverrearspk (bit 1), SubL = driversubwoofer (bit 3)
    //   FR = passengerspk (bit 4), RR = passengerrearspk (bit 5), SubR = passengersubwoofer (bit 7)
    //   Twt L = drivertwt (bit 8), Twt R = passengertwt (bit 9)
    const blockedMap = {
      fl: 1,
      rl: 2,
      sub_l: 8,
      fr: 16,
      rr: 32,
      sub_r: 128,
      twt_l: 256,
      twt_r: 512
    };
    const losFlags = data.speaker_blocked_flags;
    const validFlags = data.speaker_valid_flags;
    if (typeof losFlags === 'number' && document.getElementById('speaker-los-row')) {
      for (const [speaker, bit] of Object.entries(blockedMap)) {
        const item = document.querySelector(`.los-item[data-speaker="${speaker}"]`);
        if (!item) continue;
        const blocked = (losFlags & bit) !== 0;
        // Missing speakers (not in the vehicle) have their valid bit clear ->
        // show grey instead of a misleading green/red dot.
        const valid = typeof validFlags === 'number' && (validFlags & bit) !== 0;
        item.classList.remove('los-blocked', 'los-unknown');
        if (blocked) {
          item.classList.add('los-blocked');
        } else if (!valid) {
          item.classList.add('los-unknown');
        }
      }
    } else {
      document.querySelectorAll('.los-item').forEach(item => {
        item.classList.remove('los-blocked');
        item.classList.add('los-unknown');
      });
    }

    // Determine UDP mode locally (check if speaker data has meaningful global positions)
    const hasValidUdpSpeakers = () => {
      if (data.speaker_valid_flags !== undefined && data.speaker_valid_flags !== 0) {
        return true;
      }
      const firstSpeaker = data.speaker_driver_main;
      if (firstSpeaker && Array.isArray(firstSpeaker)) {
        const magnitude = Math.sqrt(firstSpeaker[0] ** 2 + firstSpeaker[1] ** 2 + firstSpeaker[2] ** 2);
        return magnitude > 1.0; // Global positions are typically > 1.0
      }
      return false;
    };
    window.Audio3D.isUdpMode = hasValidUdpSpeakers();

    drawSpatialPlot(data);
  };  /**
   * Load speaker mutes from backend
   */
  const loadSpeakerMutes = async () => {
    try {
      const mutes = await window.TauriBridge.getSpeakerMutes();
      speakerMutes = mutes;
      // Update UI toggles
      const toggleIds = {
        fl_muted: 'speaker-mute-fl',
        fr_muted: 'speaker-mute-fr',
        rl_muted: 'speaker-mute-rl',
        rr_muted: 'speaker-mute-rr',
        sub_l_muted: 'speaker-mute-sub-l',
        sub_r_muted: 'speaker-mute-sub-r'
      };
      for (const [key, id] of Object.entries(toggleIds)) {
        const toggle = document.getElementById(id);
        if (toggle) {
          toggle.checked = mutes[key] || false;
        }
      }
    } catch (error) {
      window.Utils.throttleLog('3d_load_mutes', console.error, ['Failed to load speaker mutes:', error], 5000, 8000);
    }
  };

  /**
   * Handle speaker mute toggle change
   */
  const handleSpeakerMuteChange = async () => {
    const mutes = {
      fl_muted: document.getElementById('speaker-mute-fl')?.checked || false,
      fr_muted: document.getElementById('speaker-mute-fr')?.checked || false,
      rl_muted: document.getElementById('speaker-mute-rl')?.checked || false,
      rr_muted: document.getElementById('speaker-mute-rr')?.checked || false,
      sub_l_muted: document.getElementById('speaker-mute-sub-l')?.checked || false,
      sub_r_muted: document.getElementById('speaker-mute-sub-r')?.checked || false
    };

    try {
      await window.TauriBridge.setSpeakerMutes(mutes);
      speakerMutes = mutes;
    } catch (error) {
      console.error('Failed to set speaker mutes:', error);
      window.Utils.logToUI('error', 'Failed to set speaker mutes');
      window.Utils.showNotification('Failed to set speaker mutes: ' + window.Utils.sanitizeErrorMessage(error), 'error');
    }
  };

  /**
   * Check if 3D audio UDP is enabled
   */
  const getIs3dAudioUdpEnabled = () => is3dAudioUdpEnabled;

  // Wrap all DOM queries and Tauri calls in DOMContentLoaded
  window.Audio3D = {
    initBandCanvas,
    initializeBandCanvases,
    initializeSpatialCanvas,
    drawBandWaveform,
    drawSpatialPlot,
    toggle3dAudioUdp,
    updateSpeakerIndicators,
    startTelemetryUpdates,
    stopTelemetryUpdates,
    handleBeamngTelemetry,
    handleBeamngCameraData,
    loadSpeakerMutes,
    handleSpeakerMuteChange,
    is3dAudioUdpEnabled: getIs3dAudioUdpEnabled
  };

  document.addEventListener('DOMContentLoaded', () => {
    initializeBandCanvases();
    initializeSpatialCanvas();
    loadSpeakerMutes();

    // Add event listener for UDP 3D audio toggle
    const udpToggle = document.getElementById('3d-audio-udp-toggle');
    if (udpToggle) {
      udpToggle.addEventListener('change', toggle3dAudioUdp);
    }

    // Load 3D enhancement toggles from backend and wire up change handlers
    // Values are camelCase (Tauri JS arg convention); the backend response is snake_case.
    const enhancementToggles = {
      'itd-toggle': 'itdEnabled',
      'head-shadow-toggle': 'headShadowEnabled',
      'sub-crossover-toggle': 'subCrossoverEnabled',
      'rear-voicing-toggle': 'rearVoicingEnabled'
    };
    const toSnakeCase = (key) => key.replace(/[A-Z]/g, (c) => '_' + c.toLowerCase());

    (async () => {
      try {
        const enhancements = await window.TauriBridge.get3dAudioEnhancements();
        for (const [elementId, key] of Object.entries(enhancementToggles)) {
          const toggle = document.getElementById(elementId);
          const field = toSnakeCase(key);
          if (toggle && enhancements[field] !== undefined) {
            toggle.checked = !!enhancements[field];
          }
        }
      } catch (error) {
        console.error('[Audio3D] Failed to load enhancement toggles:', error);
      }
    })();

    for (const [elementId, key] of Object.entries(enhancementToggles)) {
      const toggle = document.getElementById(elementId);
      if (toggle) {
        toggle.addEventListener('change', () => {
          const opts = {};
          opts[key] = toggle.checked;
          window.TauriBridge.set3dAudioEnhancements(opts)
            .catch(error => console.error('[Audio3D] Failed to set enhancement:', error));
        });
      }
    }

    // Load + wire the subwoofer volume slider
    const subVolumeSlider = document.getElementById('subwoofer-volume-slider');
    const subVolumeValue = document.getElementById('subwoofer-volume-value');
    if (subVolumeSlider) {
      (async () => {
        try {
          const state = await window.TauriBridge.get3dAudioState();
          if (state && typeof state.subwoofer_left === 'number') {
            const v = (state.subwoofer_left + state.subwoofer_right) / 2;
            subVolumeSlider.value = v;
            if (subVolumeValue) subVolumeValue.textContent = `${Math.round(v * 100)}%`;
          }
        } catch (error) {
          console.error('[Audio3D] Failed to load subwoofer volume:', error);
        }
      })();

      subVolumeSlider.addEventListener('input', () => {
        const v = parseFloat(subVolumeSlider.value);
        if (subVolumeValue) subVolumeValue.textContent = `${Math.round(v * 100)}%`;
        window.TauriBridge.setSubwooferVolume(v)
          .catch(error => console.error('[Audio3D] Failed to set subwoofer volume:', error));
      });
    }

    // Load + wire the subwoofer frequency range sliders (sub band-pass)
    const subMinSlider = document.getElementById('subwoofer-min-freq-slider');
    const subMinValue = document.getElementById('subwoofer-min-freq-value');
    const subMaxSlider = document.getElementById('subwoofer-max-freq-slider');
    const subMaxValue = document.getElementById('subwoofer-max-freq-value');
    if (subMinSlider && subMaxSlider) {
      (async () => {
        try {
          const state = await window.TauriBridge.get3dAudioState();
          if (state && typeof state.sub_low_freq === 'number') {
            subMinSlider.value = state.sub_low_freq;
            subMaxSlider.value = state.sub_high_freq;
            if (subMinValue) subMinValue.textContent = Math.round(state.sub_low_freq);
            if (subMaxValue) subMaxValue.textContent = Math.round(state.sub_high_freq);
          }
        } catch (error) {
          console.error('[Audio3D] Failed to load sub freq range:', error);
        }
      })();

      const applySubFreqRange = () => {
        let low = parseFloat(subMinSlider.value);
        let high = parseFloat(subMaxSlider.value);
        if (low >= high) {
          high = Math.min(250, low + 5);
          low = Math.min(low, high - 5);
        }
        subMinSlider.value = low;
        subMaxSlider.value = high;
        if (subMinValue) subMinValue.textContent = Math.round(low);
        if (subMaxValue) subMaxValue.textContent = Math.round(high);
        window.TauriBridge.setSubFreqRange(low, high)
          .catch(error => console.error('[Audio3D] Failed to set sub freq range:', error));
      };

      subMinSlider.addEventListener('input', applySubFreqRange);
      subMaxSlider.addEventListener('input', applySubFreqRange);
    }

    // Load + wire the tweeter volume slider
    const twtVolumeSlider = document.getElementById('tweeter-volume-slider');
    const twtVolumeValue = document.getElementById('tweeter-volume-value');
    if (twtVolumeSlider) {
      (async () => {
        try {
          const state = await window.TauriBridge.get3dAudioState();
          if (state && typeof state.tweeter_left === 'number') {
            const v = (state.tweeter_left + state.tweeter_right) / 2;
            twtVolumeSlider.value = v;
            if (twtVolumeValue) twtVolumeValue.textContent = `${Math.round(v * 100)}%`;
          }
        } catch (error) {
          console.error('[Audio3D] Failed to load tweeter volume:', error);
        }
      })();

      twtVolumeSlider.addEventListener('input', () => {
        const v = parseFloat(twtVolumeSlider.value);
        if (twtVolumeValue) twtVolumeValue.textContent = `${Math.round(v * 100)}%`;
        window.TauriBridge.setTweeterVolume(v)
          .catch(error => console.error('[Audio3D] Failed to set tweeter volume:', error));
      });
    }

    // Load + wire the tweeter frequency range sliders (min/max band-pass)
    const tweeterLowSlider = document.getElementById('tweeter-low-freq-slider');
    const tweeterLowValue = document.getElementById('tweeter-low-freq-value');
    const tweeterHighSlider = document.getElementById('tweeter-high-freq-slider');
    const tweeterHighValue = document.getElementById('tweeter-high-freq-value');
    if (tweeterLowSlider && tweeterHighSlider) {
      (async () => {
        try {
          const state = await window.TauriBridge.get3dAudioState();
          if (state && typeof state.tweeter_low_freq === 'number') {
            tweeterLowSlider.value = state.tweeter_low_freq;
            tweeterHighSlider.value = state.tweeter_high_freq;
            if (tweeterLowValue) tweeterLowValue.textContent = Math.round(state.tweeter_low_freq);
            if (tweeterHighValue) tweeterHighValue.textContent = Math.round(state.tweeter_high_freq);
          }
        } catch (error) {
          console.error('[Audio3D] Failed to load tweeter freq range:', error);
        }
      })();

      const applyTweeterFreqRange = () => {
        let low = parseFloat(tweeterLowSlider.value);
        let high = parseFloat(tweeterHighSlider.value);
        if (low >= high) {
          high = Math.min(20000, low + 100);
          low = Math.min(low, high - 100);
        }
        tweeterLowSlider.value = low;
        tweeterHighSlider.value = high;
        if (tweeterLowValue) tweeterLowValue.textContent = Math.round(low);
        if (tweeterHighValue) tweeterHighValue.textContent = Math.round(high);
        window.TauriBridge.setTweeterFreqRange(low, high)
          .catch(error => console.error('[Audio3D] Failed to set tweeter freq range:', error));
      };

      tweeterLowSlider.addEventListener('input', applyTweeterFreqRange);
      tweeterHighSlider.addEventListener('input', applyTweeterFreqRange);
    }

    // Load + wire the rattle volume slider
    const rattleVolumeSlider = document.getElementById('rattle-volume-slider');
    const rattleVolumeValue = document.getElementById('rattle-volume-value');
    if (rattleVolumeSlider) {
      (async () => {
        try {
          const state = await window.TauriBridge.get3dAudioState();
          if (state && typeof state.rattle_volume === 'number') {
            rattleVolumeSlider.value = state.rattle_volume;
            if (rattleVolumeValue) rattleVolumeValue.textContent = `${Math.round(state.rattle_volume * 100)}%`;
          }
        } catch (error) {
          console.error('[Audio3D] Failed to load rattle volume:', error);
        }
      })();

      rattleVolumeSlider.addEventListener('input', () => {
        const v = parseFloat(rattleVolumeSlider.value);
        if (rattleVolumeValue) rattleVolumeValue.textContent = `${Math.round(v * 100)}%`;
        window.TauriBridge.setRattleVolume(v)
          .catch(error => console.error('[Audio3D] Failed to set rattle volume:', error));
      });
    }

    // Load + wire the rattle threshold slider
    const rattleThresholdSlider = document.getElementById('rattle-threshold-slider');
    const rattleThresholdValue = document.getElementById('rattle-threshold-value');
    if (rattleThresholdSlider) {
      (async () => {
        try {
          const state = await window.TauriBridge.get3dAudioState();
          if (state && typeof state.rattle_threshold === 'number') {
            rattleThresholdSlider.value = state.rattle_threshold;
            if (rattleThresholdValue) rattleThresholdValue.textContent = `${Math.round(state.rattle_threshold * 100)}%`;
          }
        } catch (error) {
          console.error('[Audio3D] Failed to load rattle threshold:', error);
        }
      })();

      rattleThresholdSlider.addEventListener('input', () => {
        const v = parseFloat(rattleThresholdSlider.value);
        if (rattleThresholdValue) rattleThresholdValue.textContent = `${Math.round(v * 100)}%`;
        window.TauriBridge.setRattleThreshold(v)
          .catch(error => console.error('[Audio3D] Failed to set rattle threshold:', error));
      });
    }

    // Add event listeners for speaker mute toggles
    const muteToggleIds = ['speaker-mute-fl', 'speaker-mute-fr', 'speaker-mute-rl', 'speaker-mute-rr', 'speaker-mute-sub-l', 'speaker-mute-sub-r'];
    muteToggleIds.forEach(id => {
      const toggle = document.getElementById(id);
      if (toggle) {
        toggle.addEventListener('change', handleSpeakerMuteChange);
      }
    });

    // Resize spatial plot canvas on window resize
    let resizeTimeout;
    window.addEventListener('resize', () => {
      clearTimeout(resizeTimeout);
      resizeTimeout = setTimeout(() => {
        const canvas = document.getElementById('spatial-plot-canvas');
        if (canvas && spatialPlotCtx) {
          canvas.width = canvas.offsetWidth;
          canvas.height = canvas.offsetHeight;
        }
      }, 250);
    });
  });

})();