/**
 * Tauri Bridge Module - All Tauri API calls go here
 * This is the ONLY file allowed to call window.__TAURI__ directly
 * Every function is wrapped in try/catch with error logging
 * 
 * Fix #1 Bug #3: Export window.TauriBridge immediately (not inside DOMContentLoaded)
 * to prevent race conditions with other modules.
 */

(function() {
  'use strict';

  const TauriBridge = {
      // ============================================
      // EQ Commands
      // ============================================

      setEqBands: async function(gains) {
        try {
          return await window.__TAURI__.core.invoke('set_eq_bands', { gains });
        } catch (error) {
          console.error('[TauriBridge.setEqBands] Error:', error);
          throw error;
        }
      },

      setChannelGains: async function(leftGain, rightGain) {
        try {
          // Fix #1 Bug #1: Tauri 2.x uses camelCase in JS, auto-converts to snake_case for Rust
          return await window.__TAURI__.core.invoke('set_channel_gains', {
            leftGain: leftGain,
            rightGain: rightGain
          });
        } catch (error) {
          console.error('[TauriBridge.setChannelGains] Error:', error);
          throw error;
        }
      },

      saveEqPreset: async function(name, gains, leftGain, rightGain) {
        try {
          return await window.__TAURI__.core.invoke('save_eq_preset', {
            name: name,
            gains: gains,
            leftGain: leftGain,
            rightGain: rightGain
          });
        } catch (error) {
          console.error('[TauriBridge.saveEqPreset] Error:', error);
          throw error;
        }
      },

      loadEqPreset: async function(name) {
        try {
          return await window.__TAURI__.core.invoke('load_eq_preset', { name });
        } catch (error) {
          console.error('[TauriBridge.loadEqPreset] Error:', error);
          throw error;
        }
      },

      deleteEqPreset: async function(name) {
        try {
          return await window.__TAURI__.core.invoke('delete_eq_preset', { name });
        } catch (error) {
          console.error('[TauriBridge.deleteEqPreset] Error:', error);
          throw error;
        }
      },

      getPresetNames: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_preset_names');
        } catch (error) {
          console.error('[TauriBridge.getPresetNames] Error:', error);
          throw error;
        }
      },

      uploadPresetToDiscord: async function(presetName, gains, leftGain, rightGain, userName) {
        try {
          return await window.__TAURI__.core.invoke('upload_preset_to_discord', {
            presetName: presetName,
            gains: gains,
            leftGain: leftGain,
            rightGain: rightGain,
            userName: userName
          });
        } catch (error) {
          console.error('[TauriBridge.uploadPresetToDiscord] Error:', error);
          throw error;
        }
      },

      // ============================================
      // Audio Device Commands
      // ============================================

      getAudioInputDevices: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_audio_input_devices');
        } catch (error) {
          console.error('[TauriBridge.getAudioInputDevices] Error:', error);
          throw error;
        }
      },

      getAudioOutputDevices: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_audio_output_devices');
        } catch (error) {
          console.error('[TauriBridge.getAudioOutputDevices] Error:', error);
          throw error;
        }
      },

      startAudioCapture: async function(inputDeviceName, outputDeviceName) {
        try {
          return await window.__TAURI__.core.invoke('start_audio_capture', {
            inputDeviceName: inputDeviceName,
            outputDeviceName: outputDeviceName
          });
        } catch (error) {
          console.error('[TauriBridge.startAudioCapture] Error:', error);
          throw error;
        }
      },

      stopAudioCapture: async function() {
        try {
          return await window.__TAURI__.core.invoke('stop_audio_capture');
        } catch (error) {
          console.error('[TauriBridge.stopAudioCapture] Error:', error);
          throw error;
        }
      },

      saveAppConfig: async function(inputDevice, outputDevice) {
        try {
          // Fix #1 Bug #1: Use camelCase - Tauri auto-converts to snake_case
          return await window.__TAURI__.core.invoke('save_app_config', {
            inputDevice: inputDevice,
            outputDevice: outputDevice
          });
        } catch (error) {
          console.error('[TauriBridge.saveAppConfig] Error:', error);
          throw error;
        }
      },

      loadAppConfig: async function() {
        try {
          return await window.__TAURI__.core.invoke('load_app_config');
        } catch (error) {
          console.error('[TauriBridge.loadAppConfig] Error:', error);
          throw error;
        }
      },

      // ============================================
      // 3D Audio Commands
      // ============================================

      get3dAudioState: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_3d_audio_state');
        } catch (error) {
          console.error('[TauriBridge.get3dAudioState] Error:', error);
          throw error;
        }
      },

      set3dAudioUdpEnabled: async function(enabled) {
        try {
          return await window.__TAURI__.core.invoke('set_3d_audio_udp_enabled', { enabled });
        } catch (error) {
          console.error('[TauriBridge.set3dAudioUdpEnabled] Error:', error);
          throw error;
        }
      },

      is3dAudioUdpEnabled: async function() {
        try {
          return await window.__TAURI__.core.invoke('is_3d_audio_udp_enabled');
        } catch (error) {
          console.error('[TauriBridge.is3dAudioUdpEnabled] Error:', error);
          throw error;
        }
      },

      get3dAudioEnhancements: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_3d_audio_enhancements');
        } catch (error) {
          console.error('[TauriBridge.get3dAudioEnhancements] Error:', error);
          throw error;
        }
      },

      set3dAudioEnhancements: async function(opts) {
        try {
          // Tauri maps camelCase JS arg keys to snake_case Rust params
          const payload = {};
          if (opts.itdEnabled !== undefined) payload.itdEnabled = opts.itdEnabled;
          if (opts.headShadowEnabled !== undefined) payload.headShadowEnabled = opts.headShadowEnabled;
          if (opts.subCrossoverEnabled !== undefined) payload.subCrossoverEnabled = opts.subCrossoverEnabled;
          if (opts.rearVoicingEnabled !== undefined) payload.rearVoicingEnabled = opts.rearVoicingEnabled;
          return await window.__TAURI__.core.invoke('set_3d_audio_enhancements', payload);
        } catch (error) {
          console.error('[TauriBridge.set3dAudioEnhancements] Error:', error);
          throw error;
        }
      },

      setSubwooferVolume: async function(volume) {
        try {
          return await window.__TAURI__.core.invoke('set_subwoofer_volume', { volume });
        } catch (error) {
          console.error('[TauriBridge.setSubwooferVolume] Error:', error);
          throw error;
        }
      },

      setSubFreqRange: async function(lowFreq, highFreq) {
        try {
          return await window.__TAURI__.core.invoke('set_sub_freq_range', {
            lowFreq,
            highFreq
          });
        } catch (error) {
          console.error('[TauriBridge.setSubFreqRange] Error:', error);
          throw error;
        }
      },

      setTweeterVolume: async function(volume) {
        try {
          return await window.__TAURI__.core.invoke('set_tweeter_volume', { volume });
        } catch (error) {
          console.error('[TauriBridge.setTweeterVolume] Error:', error);
          throw error;
        }
      },

      setTweeterFreqRange: async function(lowFreq, highFreq) {
        try {
          return await window.__TAURI__.core.invoke('set_tweeter_freq_range', {
            lowFreq,
            highFreq
          });
        } catch (error) {
          console.error('[TauriBridge.setTweeterFreqRange] Error:', error);
          throw error;
        }
      },

      setRattleVolume: async function(volume) {
        try {
          return await window.__TAURI__.core.invoke('set_rattle_volume', { volume });
        } catch (error) {
          console.error('[TauriBridge.setRattleVolume] Error:', error);
          throw error;
        }
      },

      setRattleThreshold: async function(threshold) {
        try {
          return await window.__TAURI__.core.invoke('set_rattle_threshold', { threshold });
        } catch (error) {
          console.error('[TauriBridge.setRattleThreshold] Error:', error);
          throw error;
        }
      },

      getSpeakerMutes: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_speaker_mutes');
        } catch (error) {
          console.error('[TauriBridge.getSpeakerMutes] Error:', error);
          throw error;
        }
      },

      setSpeakerMutes: async function(config) {
        try {
          return await window.__TAURI__.core.invoke('set_speaker_mutes', { config });
        } catch (error) {
          console.error('[TauriBridge.setSpeakerMutes] Error:', error);
          throw error;
        }
      },

      // ============================================
      // Media Info Commands
      // ============================================

      getMediaInfo: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_media_info');
        } catch (error) {
          console.error('[TauriBridge.getMediaInfo] Error:', error);
          throw error;
        }
      },

      mediaPlayPause: async function() {
        try {
          return await window.__TAURI__.core.invoke('media_play_pause');
        } catch (error) {
          console.error('[TauriBridge.mediaPlayPause] Error:', error);
          throw error;
        }
      },

      mediaNext: async function() {
        try {
          return await window.__TAURI__.core.invoke('media_next');
        } catch (error) {
          console.error('[TauriBridge.mediaNext] Error:', error);
          throw error;
        }
      },

      mediaPrevious: async function() {
        try {
          return await window.__TAURI__.core.invoke('media_previous');
        } catch (error) {
          console.error('[TauriBridge.mediaPrevious] Error:', error);
          throw error;
        }
      },

      getLatestCameraDataCommand: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_latest_camera_data_command');
        } catch (error) {
          console.error('[TauriBridge.getLatestCameraDataCommand] Error:', error);
          throw error;
        }
      },

      // ============================================
      // UDP Status Command
      // ============================================

      getUdpStatus: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_udp_status');
        } catch (error) {
          console.error('[TauriBridge.getUdpStatus] Error:', error);
          throw error;
        }
      },

      // ============================================
      // Electrics UDP Commands
      // ============================================

      getElectricsUdpStatus: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_electrics_udp_status');
        } catch (error) {
          console.error('[TauriBridge.getElectricsUdpStatus] Error:', error);
          throw error;
        }
      },

      startElectricsUdpListener: async function() {
        try {
          return await window.__TAURI__.core.invoke('start_electrics_udp_listener');
        } catch (error) {
          console.error('[TauriBridge.startElectricsUdpListener] Error:', error);
          throw error;
        }
      },

      stopElectricsUdpListener: async function() {
        try {
          return await window.__TAURI__.core.invoke('stop_electrics_udp_listener');
        } catch (error) {
          console.error('[TauriBridge.stopElectricsUdpListener] Error:', error);
          throw error;
        }
      },

      isElectricsUdpEnabled: async function() {
        try {
          return await window.__TAURI__.core.invoke('is_electrics_udp_enabled');
        } catch (error) {
          console.error('[TauriBridge.isElectricsUdpEnabled] Error:', error);
          throw error;
        }
      },

      // ============================================
      // Visualizer Transport (HTTP vs WebSocket)
      // ============================================

      setVisualizerWsMode: async function(enabled) {
        try {
          return await window.__TAURI__.core.invoke('visualizer_set_ws_mode', { enabled });
        } catch (error) {
          console.error('[TauriBridge.setVisualizerWsMode] Error:', error);
          throw error;
        }
      },

      getElectricsValues: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_electrics_values');
        } catch (error) {
          console.error('[TauriBridge.getElectricsValues] Error:', error);
          throw error;
        }
      },

      // ============================================
      // Discord Webhook Commands
      // ============================================

      setDiscordWebhookUrl: async function(url) {
        try {
          return await window.__TAURI__.core.invoke('set_discord_webhook_url', { url });
        } catch (error) {
          console.error('[TauriBridge.setDiscordWebhookUrl] Error:', error);
          throw error;
        }
      },

      // ============================================
      // BeamNG Mods Commands
      // ============================================

      browseBeamNgMods: async function() {
        try {
          return await window.__TAURI__.core.invoke('browse_beamng_mods');
        } catch (error) {
          console.error('[TauriBridge.browseBeamNgMods] Error:', error);
          throw error;
        }
      },

      saveBeamNgModsFolder: async function() {
        try {
          return await window.__TAURI__.core.invoke('save_beamng_mods_folder');
        } catch (error) {
          console.error('[TauriBridge.saveBeamNgModsFolder] Error:', error);
          throw error;
        }
      },

      checkModStatus: async function() {
        try {
          return await window.__TAURI__.core.invoke('check_mod_status');
        } catch (error) {
          console.error('[TauriBridge.checkModStatus] Error:', error);
          throw error;
        }
      },

      // ============================================
      // Event Listeners
      // ============================================

      // Amplifier/subwoofer telemetry from the BeamNG mod
      // (ampSimGE -> ws/UDP -> amp_state.rs -> 'amp_state_updated' event).
      // Cached in TauriBridge.ampState.last for synchronous access.
      ampState: { last: null },

      onAmpState: async function(handler) {
        try {
          return await this.listen('amp_state_updated', (event) => {
            this.ampState.last = event.payload;
            if (handler) handler(event.payload);
          });
        } catch (error) {
          console.error('[TauriBridge.onAmpState] Error:', error);
          throw error;
        }
      },

      getAmpState: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_amp_state');
        } catch (error) {
          console.error('[TauriBridge.getAmpState] Error:', error);
          throw error;
        }
      },

      getAmpStateStatus: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_amp_state_status');
        } catch (error) {
          console.error('[TauriBridge.getAmpStateStatus] Error:', error);
          throw error;
        }
      },

      // Manual amp/sub/box overrides (app sliders beat the game packets)
      setAmpOverride: async function(key, value) {
        try {
          return await window.__TAURI__.core.invoke('set_amp_override', { key, value });
        } catch (error) {
          console.error('[TauriBridge.setAmpOverride] Error:', error);
          throw error;
        }
      },

      clearAmpOverride: async function(key) {
        try {
          return await window.__TAURI__.core.invoke('clear_amp_override', { key });
        } catch (error) {
          console.error('[TauriBridge.clearAmpOverride] Error:', error);
          throw error;
        }
      },

      clearAmpOverrides: async function() {
        try {
          return await window.__TAURI__.core.invoke('clear_amp_overrides');
        } catch (error) {
          console.error('[TauriBridge.clearAmpOverrides] Error:', error);
          throw error;
        }
      },

      getAmpOverrides: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_amp_overrides');
        } catch (error) {
          console.error('[TauriBridge.getAmpOverrides] Error:', error);
          throw error;
        }
      },

      listen: async function(eventName, handler) {
        try {
          return await window.__TAURI__.event.listen(eventName, handler);
        } catch (error) {
          console.error(`[TauriBridge.listen] Error listening to '${eventName}':`, error);
          throw error;
        }
      },

      emit: async function(eventName, payload) {
        try {
          return await window.__TAURI__.event.emit(eventName, payload);
        } catch (error) {
          console.error(`[TauriBridge.emit] Error emitting '${eventName}':`, error);
          throw error;
        }
      },

      // ============================================
      // Session Stats Commands
      // ============================================

      getSessionStats: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_session_stats');
        } catch (error) {
          console.error('[TauriBridge.getSessionStats] Error:', error);
          throw error;
        }
      },

      getLifetimeStats: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_lifetime_stats');
        } catch (error) {
          console.error('[TauriBridge.getLifetimeStats] Error:', error);
          throw error;
        }
      },

      resetLifetimeStats: async function() {
        try {
          return await window.__TAURI__.core.invoke('reset_lifetime_stats');
        } catch (error) {
          console.error('[TauriBridge.resetLifetimeStats] Error:', error);
          throw error;
        }
      },

      getSessionLogFiles: async function() {
        try {
          return await window.__TAURI__.core.invoke('get_session_log_files');
        } catch (error) {
          console.error('[TauriBridge.getSessionLogFiles] Error:', error);
          throw error;
        }
      },

      readSessionLog: async function(filename) {
        try {
          return await window.__TAURI__.core.invoke('read_session_log', { filename });
        } catch (error) {
          console.error('[TauriBridge.readSessionLog] Error:', error);
          throw error;
        }
      },

      exportSessionLog: async function(filename) {
        try {
          return await window.__TAURI__.core.invoke('export_session_log', { filename });
        } catch (error) {
          console.error('[TauriBridge.exportSessionLog] Error:', error);
          throw error;
        }
      },

      // ============================================
      // Embedded Opera Commands (Win32 SetParent)
      // ============================================

      findOperaExe: async function() {
        try {
          return await window.__TAURI__.core.invoke('find_opera_exe');
        } catch (error) {
          console.error('[TauriBridge.findOperaExe] Error:', error);
          return null;
        }
      },

      launchOperaEmbed: async function(exe) {
        try {
          return await window.__TAURI__.core.invoke('launch_opera_embed', { exe: exe || null });
        } catch (error) {
          console.error('[TauriBridge.launchOperaEmbed] Error:', error);
          throw error;
        }
      },

      resizeEmbeddedApp: async function(x, y, w, h, scale, vw, vh) {
        try {
          return await window.__TAURI__.core.invoke('resize_embedded_app', {
            x, y, w, h,
            scale: scale || window.devicePixelRatio || 1,
            vw: vw || window.innerWidth || 0,
            vh: vh || window.innerHeight || 0
          });
        } catch (error) {
          console.error('[TauriBridge.resizeEmbeddedApp] Error:', error);
          throw error;
        }
      },

      closeEmbeddedApp: async function() {
        try {
          return await window.__TAURI__.core.invoke('close_embedded_app');
        } catch (error) {
          console.error('[TauriBridge.closeEmbeddedApp] Error:', error);
          throw error;
        }
      },

      focusEmbeddedApp: async function() {
        try {
          return await window.__TAURI__.core.invoke('focus_embedded_app');
        } catch (error) {
          console.error('[TauriBridge.focusEmbeddedApp] Error:', error);
          throw error;
        }
      },

      // ============================================
      // Embedded Opera Stream Commands (WS JPEG)
      // ============================================

      startOperaStream: async function(port, fps, quality) {
        try {
          return await window.__TAURI__.core.invoke('start_opera_stream', { port, fps, quality });
        } catch (error) {
          console.error('[TauriBridge.startOperaStream] Error:', error);
          throw error;
        }
      },

      stopOperaStream: async function() {
        try {
          return await window.__TAURI__.core.invoke('stop_opera_stream');
        } catch (error) {
          console.error('[TauriBridge.stopOperaStream] Error:', error);
          throw error;
        }
      },

      operaStreamStatus: async function() {
        try {
          return await window.__TAURI__.core.invoke('stream_status');
        } catch (error) {
          console.error('[TauriBridge.operaStreamStatus] Error:', error);
          throw error;
        }
      },

      // ============================================
      // Window (aspect lock + sizing)
      // ============================================

      currentWindow: function() {
        try {
          const ns = window.__TAURI__ && window.__TAURI__.window;
          if (!ns || !ns.getCurrentWindow) return null;
          return ns.getCurrentWindow();
        } catch (error) {
          console.error('[TauriBridge.currentWindow] Error:', error);
          return null;
        }
      },

      windowSizes: async function() {
        try {
          const w = TauriBridge.currentWindow();
          if (!w) return null;
          const [inner, outer] = await Promise.all([w.innerSize(), w.outerSize()]);
          return {
            inner: { width: inner.width, height: inner.height },
            outer: { width: outer.width, height: outer.height }
          };
        } catch (error) {
          console.error('[TauriBridge.windowSizes] Error:', error);
          return null;
        }
      },

      setWindowSize: async function(width, height) {
        try {
          const w = TauriBridge.currentWindow();
          if (!w) return;
          const ns = window.__TAURI__.window;
          await w.setSize(new ns.PhysicalSize(Math.round(width), Math.round(height)));
        } catch (error) {
          console.error('[TauriBridge.setWindowSize] Error:', error);
        }
      },

      windowState: async function() {
        try {
          const w = TauriBridge.currentWindow();
          if (!w) return null;
          const [maximized, fullscreen, minimized] = await Promise.all([
            w.isMaximized(), w.isFullscreen(), w.isMinimized()
          ]);
          return { maximized, fullscreen, minimized };
        } catch (error) {
          console.error('[TauriBridge.windowState] Error:', error);
          return null;
        }
      },

      onWindowResized: async function(handler) {
        try {
          const w = TauriBridge.currentWindow();
          if (!w) return null;
          return await w.onResized((event) => handler(event.payload));
        } catch (error) {
          console.error('[TauriBridge.onWindowResized] Error:', error);
          return null;
        }
      },

      // ============================================
      // System
      // ============================================

      getExeIcon: async function(exe) {
        try {
          return await window.__TAURI__.core.invoke('get_exe_icon', { exe });
        } catch (error) {
          return null;
        }
      },

      launchEmbeddedApp: async function(exe, args, appId, mcpEnabled, mcpHost, mcpPort) {
        try {
          return await window.__TAURI__.core.invoke('launch_embedded_app', { exe, args: args || null, appId, mcpEnabled, mcpHost, mcpPort });
        } catch (error) {
          console.error('[TauriBridge.launchEmbeddedApp] Error:', error);
          throw error;
        }
      },

      embedLaunchedApp: async function() {
        try {
          return await window.__TAURI__.core.invoke('embed_launched_app');
        } catch (error) {
          console.error('[TauriBridge.embedLaunchedApp] Error:', error);
          throw error;
        }
      },

      testMcpConnection: async function(host, port) {
        try {
          return await window.__TAURI__.core.invoke('mcp_test_connection', { host, port });
        } catch (error) {
          console.error('[TauriBridge.testMcpConnection] Error:', error);
          throw error;
        }
      }
    };

    // Fix #1 Bug #3: Export immediately, not inside DOMContentLoaded
    // This prevents race conditions with other modules that call TauriBridge
    window.TauriBridge = TauriBridge;

    console.log('[TauriBridge] Initialized successfully');
})();
