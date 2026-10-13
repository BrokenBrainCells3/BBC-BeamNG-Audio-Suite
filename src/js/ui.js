/**
 * UI Module - Main UI initialization and event delegation
 * Uses window.TauriBridge for all Tauri API calls
 * Uses window.Utils for utility functions
 * Uses window.Audio3D, window.ChatSystem for features
 */

(function() {
  'use strict';

  // Constants
  const userNameKey = "audio-viz-user-name";
  const themeColorKey = "audio-viz-theme-color";
  const defaultThemeColor = "#58c2ff";

  // EQ State
  const eqBands = new Array(16).fill(0);
  let leftChannelGain = 0;
  let rightChannelGain = 0;

  // Electrics UDP State
  let electricsState = {
    udpEnabled: false,
    eqBandValues: new Array(15).fill(0),  // Raw values from -16 to 16
    volumeValue: 0
  };
  let electricsValuesDebouncer = null;
  const ELECTRICS_UPDATE_DELAY_MS = 50;

  // Debounce timers
  const eqUpdateDebouncers = {};
  const channelGainDebouncers = {};
  const EQ_UPDATE_DELAY_MS = 50;

  // Media info interval
  let mediaInfoInterval = null;
  let lastMediaIsPlaying = false;

  // Visualizer animation frame
  let visualizerAnimationId = null;

  // Camera poll interval
  let cameraPollInterval = null;

  /**
   * Apply theme color to CSS variables
   */
  const applyThemeColor = (color) => {
    const hoverColor = window.Utils.adjustColorBrightness(color, -20);
    const glowColor = window.Utils.hexToRgba(color, 0.3);
    const lightColor = window.Utils.hexToRgba(color, 0.1);
    const accentColor = window.Utils.hexToRgba(color, 0.2);

    const hsl = window.Utils.hexToHsl(color);
    const bgDark = `hsl(${hsl.h}, 15%, 15%)`;
    const bgCard = `hsl(${hsl.h}, 15%, 18%)`;
    const bgInput = `hsl(${hsl.h}, 15%, 22%)`;
    const borderColor = `hsl(${hsl.h}, 15%, 28%)`;
    const gradientStart = `hsl(${hsl.h}, 15%, 15%)`;
    const gradientEnd = `hsl(${hsl.h}, 15%, 18%)`;

    document.documentElement.style.setProperty('--primary-color', color);
    document.documentElement.style.setProperty('--primary-hover', hoverColor);
    document.documentElement.style.setProperty('--primary-glow', glowColor);
    document.documentElement.style.setProperty('--primary-light', lightColor);
    document.documentElement.style.setProperty('--primary-accent', accentColor);
    document.documentElement.style.setProperty('--bg-dark', bgDark);
    document.documentElement.style.setProperty('--bg-card', bgCard);
    document.documentElement.style.setProperty('--bg-input', bgInput);
    document.documentElement.style.setProperty('--border-color', borderColor);
    document.documentElement.style.setProperty('--gradient-start', gradientStart);
    document.documentElement.style.setProperty('--gradient-end', gradientEnd);
  };

  /**
   * Save theme color
   */
  const saveThemeColor = (color) => {
    localStorage.setItem(themeColorKey, color);
    applyThemeColor(color);
  };

  /**
   * Load theme color
   */
  const loadThemeColor = () => {
    const savedColor = localStorage.getItem(themeColorKey) || defaultThemeColor;
    applyThemeColor(savedColor);
    const colorInput = document.getElementById("theme-color-input");
    if (colorInput) colorInput.value = savedColor;
  };

  /**
   * Reset theme color
   */
  const resetThemeColor = () => {
    localStorage.removeItem(themeColorKey);
    applyThemeColor(defaultThemeColor);
    const colorInput = document.getElementById("theme-color-input");
    if (colorInput) colorInput.value = defaultThemeColor;
    window.Utils.showNotification("Theme color reset to default", "info");
  };

  /**
   * Get stored user name
   */
  const getUserName = () => localStorage.getItem(userNameKey);

  /**
   * Set profile name field value
   */
  const setProfileNameField = (value) => {
    const input = document.querySelector("#profile-name");
    if (input) input.value = value ?? "";
  };

  /**
   * Open side menu
   */
  const openSideMenu = () => {
    const menu = document.getElementById("side-menu");
    if (menu) menu.style.width = "250px";
  };

  /**
   * Close side menu
   */
  const closeSideMenu = () => {
    const menu = document.getElementById("side-menu");
    if (menu) menu.style.width = "0";
  };

  /**
   * Scroll functions
   */
  const scrollToBeamNgPanel = () => {
    const panel = document.querySelector("#beamng-panel");
    if (panel) panel.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  const scrollToEQPanel = () => {
    const panel = document.querySelector("#eq-panel");
    if (panel) panel.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  const scrollToSettingsPanel = () => {
    const panel = document.querySelector("#settings-panel");
    if (panel) panel.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  const scrollToLogPanel = () => {
    const panel = document.querySelector("#log-panel");
    if (panel) panel.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  const scrollToSessionStatsPanel = () => {
    const panel = document.querySelector("#session-stats-panel");
    if (panel) {
      panel.scrollIntoView({ behavior: "smooth", block: "start" });
      // Refresh session stats when panel is shown
      if (window.SessionStats) {
        window.SessionStats.refreshStats();
      }
    }
  };

  const scrollToAmpPanel = () => {
    const panel = document.querySelector("#amp-panel");
    if (panel) panel.scrollIntoView({ behavior: "smooth", block: "start" });
  };

  /**
   * Reset settings controls
   */
  const resetSettingsControls = () => {
    const outputDevice = document.querySelector("#output-device");
    if (outputDevice) outputDevice.selectedIndex = 0;
    setProfileNameField(getUserName());
  };

  /**
   * Update EQ value
   */
  const updateEQValue = async (bandIndex, value) => {
    // Block manual control when electrics UDP is enabled
    if (electricsState.udpEnabled) {
      window.Utils.showNotification('Electrics UDP active - disable to adjust manually', 'warning');
      return;
    }

    eqBands[bandIndex] = parseFloat(value);
    const slider = document.querySelector(`.eq-slider[data-band="${bandIndex}"]`);
    if (slider) {
      const valueDisplay = slider.closest(".eq-band").querySelector(".eq-value");
      if (valueDisplay) valueDisplay.textContent = `${parseFloat(value).toFixed(1)} dB`;
    }

    if (eqUpdateDebouncers[bandIndex]) {
      clearTimeout(eqUpdateDebouncers[bandIndex]);
    }
    eqUpdateDebouncers[bandIndex] = setTimeout(async () => {
      try {
        await window.TauriBridge.setEqBands(eqBands);
      } catch (error) {
        console.error("Failed to set EQ bands:", error);
      }
    }, EQ_UPDATE_DELAY_MS);
  };

  /**
   * Reset EQ
   */
  const resetEQ = async () => {
    for (let i = 0; i < 16; i++) {
      eqBands[i] = 0;
      const slider = document.querySelector(`.eq-slider[data-band="${i}"]`);
      if (slider) {
        slider.value = 0;
        const valueDisplay = slider.closest(".eq-band").querySelector(".eq-value");
        if (valueDisplay) valueDisplay.textContent = "0.0 dB";
      }
    }
    try {
      await window.TauriBridge.setEqBands(eqBands);
      window.Utils.showNotification("EQ reset to default values", "info");
    } catch (error) {
      console.error("Failed to reset EQ:", error);
      window.Utils.showNotification("Failed to reset EQ: " + window.Utils.sanitizeErrorMessage(error), "error");
    }
  };

  /**
   * Update channel gain
   */
  const updateChannelGain = async (channel, value) => {
    // Block manual control when electrics UDP is enabled
    if (electricsState.udpEnabled) {
      window.Utils.showNotification('Electrics UDP active - disable to adjust manually', 'warning');
      return;
    }

    if (channel === 'left') {
      leftChannelGain = parseFloat(value);
      const display = document.querySelector('#left-gain').closest('.channel-gain').querySelector('.gain-value');
      if (display) display.textContent = `${parseFloat(value).toFixed(1)} dB`;
    } else if (channel === 'right') {
      rightChannelGain = parseFloat(value);
      const display = document.querySelector('#right-gain').closest('.channel-gain').querySelector('.gain-value');
      if (display) display.textContent = `${parseFloat(value).toFixed(1)} dB`;
    }

    const debouncerKey = channel;
    if (channelGainDebouncers[debouncerKey]) {
      clearTimeout(channelGainDebouncers[debouncerKey]);
    }
    channelGainDebouncers[debouncerKey] = setTimeout(async () => {
      try {
        await window.TauriBridge.setChannelGains(leftChannelGain, rightChannelGain);
      } catch (error) {
        console.error("Failed to set channel gains:", error);
      }
    }, EQ_UPDATE_DELAY_MS);
  };

  /**
   * Map electrics value (-16 to 16) to dB (-16 to 16) - 1:1 mapping
   */
  const mapElectricsToDb = (electricValue) => {
    // Direct 1:1 mapping - electrics value equals dB value
    return Math.max(-16, Math.min(16, electricValue));
  };

  /**
   * Update EQ from electrics UDP values
   */
  const updateEqFromElectrics = async (eqBandValues, volumeValue) => {
    // Map each electrics band to dB (bands 0-14: EQ bands)
    for (let i = 0; i < 15; i++) {
      const dbValue = mapElectricsToDb(eqBandValues[i]);
      eqBands[i] = dbValue;  // Keep float precision
      const slider = document.querySelector(`.eq-slider[data-band="${i}"]`);
      if (slider) {
        slider.value = dbValue.toFixed(1);
        const valueDisplay = slider.closest(".eq-band").querySelector(".eq-value");
        if (valueDisplay) valueDisplay.textContent = `${dbValue.toFixed(1)} dB`;
      }
    }

    // Map volume electrics to dB and update band 15 (Volume slider)
    const volumeDb = mapElectricsToDb(volumeValue);
    eqBands[15] = volumeDb;
    const volumeSlider = document.querySelector(`.eq-slider[data-band="15"]`);
    if (volumeSlider) {
      volumeSlider.value = volumeDb.toFixed(1);
      const volumeDisplay = volumeSlider.closest(".eq-band").querySelector(".eq-value");
      if (volumeDisplay) volumeDisplay.textContent = `${volumeDb.toFixed(1)} dB`;
    }

    // Immediately update Rust with new values (no debounce for real-time electrics)
    try {
      console.log('[Electrics] Sending EQ bands to Rust:', eqBands);
      await window.TauriBridge.setEqBands(eqBands);
    } catch (error) {
      console.error("Failed to set EQ bands from electrics:", error);
    }
  };

  /**
   * Toggle electrics UDP control
   */
  const toggleElectricsUdp = async (enabled) => {
    electricsState.udpEnabled = enabled;
    try {
      if (enabled) {
        await window.TauriBridge.startElectricsUdpListener();
        window.Utils.showNotification("Electrics UDP control enabled", "success");
      } else {
        await window.TauriBridge.stopElectricsUdpListener();
        window.Utils.showNotification("Electrics UDP control disabled", "info");
      }
    } catch (error) {
      console.error("Failed to toggle electrics UDP:", error);
      window.Utils.showNotification("Failed to toggle electrics UDP: " + window.Utils.sanitizeErrorMessage(error), "error");
      electricsState.udpEnabled = !enabled;
    }
  };

  /**
   * Save EQ preset
   */
  const saveEqPreset = async (name) => {
    if (!name || name.trim() === "") {
      window.Utils.showNotification("Please enter a preset name", "warning");
      return;
    }
    try {
      await window.TauriBridge.saveEqPreset(name.trim(), eqBands, leftChannelGain, rightChannelGain);
      await loadPresetNames();
      window.Utils.showNotification(`Preset "${name}" saved successfully!`, "success");
    } catch (error) {
      console.error("Failed to save preset:", error);
      window.Utils.showNotification(`Failed to save preset: ${window.Utils.sanitizeErrorMessage(error)}`, "error");
    }
  };

  /**
   * Load EQ preset
   */
  const loadEqPreset = async (name) => {
    if (!name) return;
    try {
      const preset = await window.TauriBridge.loadEqPreset(name);
      for (let i = 0; i < 16; i++) {
        eqBands[i] = preset.gains[i];
        const slider = document.querySelector(`.eq-slider[data-band="${i}"]`);
        if (slider) {
          slider.value = preset.gains[i];
          const valueDisplay = slider.closest(".eq-band").querySelector(".eq-value");
          if (valueDisplay) valueDisplay.textContent = `${preset.gains[i]} dB`;
        }
      }
      const leftSlider = document.querySelector("#left-gain");
      const rightSlider = document.querySelector("#right-gain");
      if (leftSlider) {
        leftSlider.value = preset.left_gain;
        leftChannelGain = preset.left_gain;
        const leftDisplay = leftSlider.closest('.channel-gain').querySelector('.gain-value');
        if (leftDisplay) leftDisplay.textContent = `${preset.left_gain} dB`;
      }
      if (rightSlider) {
        rightSlider.value = preset.right_gain;
        rightChannelGain = preset.right_gain;
        const rightDisplay = rightSlider.closest('.channel-gain').querySelector('.gain-value');
        if (rightDisplay) rightDisplay.textContent = `${preset.right_gain} dB`;
      }
      await window.TauriBridge.setEqBands(eqBands);
      window.Utils.showNotification(`Preset "${name}" loaded successfully!`, "success");
    } catch (error) {
      console.error("Failed to load preset:", error);
      window.Utils.showNotification(`Failed to load preset: ${window.Utils.sanitizeErrorMessage(error)}`, "error");
    }
  };

  /**
   * Delete EQ preset
   */
  const deleteEqPreset = async (name) => {
    if (!name) return;
    if (!confirm(`Delete preset "${name}"?`)) return;
    try {
      await window.TauriBridge.deleteEqPreset(name);
      await loadPresetNames();
      const select = document.querySelector("#preset-select");
      if (select) select.value = "";
      window.Utils.showNotification(`Preset "${name}" deleted successfully!`, "success");
    } catch (error) {
      console.error("Failed to delete preset:", error);
      window.Utils.showNotification(`Failed to delete preset: ${window.Utils.sanitizeErrorMessage(error)}`, "error");
    }
  };

  /**
   * Upload EQ preset to Discord
   */
  const uploadEqPresetToDiscord = async (presetName) => {
    let preset = await window.TauriBridge.loadEqPreset(presetName).catch(() => null);

    if (!preset) {
      const gains = [];
      for (let i = 0; i < 16; i++) {
        const slider = document.querySelector(`.eq-slider[data-band="${i}"]`);
        gains.push(slider ? parseInt(slider.value, 10) : 0);
      }
      const leftSlider = document.querySelector("#left-gain");
      const rightSlider = document.querySelector("#right-gain");
      const leftGain = leftSlider ? parseInt(leftSlider.value, 10) : 0;
      const rightGain = rightSlider ? parseInt(rightSlider.value, 10) : 0;
      preset = { gains, left_gain: leftGain, right_gain: rightGain };
    }

    const userName = getUserName() || "Unknown User";

    try {
      const result = await window.TauriBridge.uploadPresetToDiscord(
        presetName, preset.gains, preset.left_gain, preset.right_gain, userName
      );
      window.Utils.showNotification(result, "success");
    } catch (error) {
      console.error("Failed to upload preset:", error);
      window.Utils.showNotification(`Failed to upload preset to Discord: ${window.Utils.sanitizeErrorMessage(error)}`, "error");
    }
  };

  /**
   * Load preset names
   */
  const loadPresetNames = async () => {
    const select = document.querySelector("#preset-select");
    if (!select) return;
    try {
      const names = await window.TauriBridge.getPresetNames();
      select.innerHTML = '<option value="">Load Preset...</option>';
      names.forEach(name => {
        const option = document.createElement("option");
        option.value = name;
        option.textContent = name;
        select.appendChild(option);
      });
    } catch (error) {
      console.error("Failed to load preset names:", error);
    }
  };

  /**
   * Save app config
   */
  const saveAppConfig = async () => {
    const inputSelect = document.querySelector("#input-device");
    const outputSelect = document.querySelector("#output-device");
    try {
      await window.TauriBridge.saveAppConfig(
        inputSelect?.value || null,
        outputSelect?.value || null
      );
      window.Utils.showNotification("Settings saved", "success");
    } catch (error) {
      console.error("Failed to save config:", error);
      window.Utils.showNotification("Failed to save settings: " + window.Utils.sanitizeErrorMessage(error), "error");
    }
  };

  /**
   * Load app config
   */
  const loadAppConfig = async () => {
    try {
      const config = await window.TauriBridge.loadAppConfig();
      if (config.last_input_device) {
        const inputSelect = document.querySelector("#input-device");
        if (inputSelect) {
          setTimeout(() => {
            inputSelect.value = config.last_input_device;
          }, 100);
        }
      }
      if (config.last_output_device) {
        const outputSelect = document.querySelector("#output-device");
        if (outputSelect) {
          setTimeout(() => {
            outputSelect.value = config.last_output_device;
          }, 100);
        }
      }
    } catch (error) {
      console.error("Failed to load config:", error);
    }
  };

  /**
   * Populate device dropdown
   */
  const populateDeviceDropdown = async (selector, command) => {
    const select = document.querySelector(selector);
    if (!select) return;

    select.innerHTML = "";

    try {
      const devices = command === 'get_audio_input_devices'
        ? await window.TauriBridge.getAudioInputDevices()
        : await window.TauriBridge.getAudioOutputDevices();

      if (devices.length === 0) {
        return;
      }

      devices.forEach((device) => {
        const option = document.createElement("option");
        option.value = device;
        option.textContent = device;
        select.appendChild(option);
      });
    } catch (error) {
      console.error(`Failed to get devices:`, error);
    }
  };

  /**
   * Populate audio input devices
   */
  const populateAudioInputDevices = async () => {
    await populateDeviceDropdown("#input-device", "get_audio_input_devices");
  };

  /**
   * Populate audio output devices
   */
  const populateAudioOutputDevices = async () => {
    await populateDeviceDropdown("#output-device", "get_audio_output_devices");
  };

  /**
   * Start audio capture
   */
  const startAudioCapture = async () => {
    const inputSelect = document.querySelector("#input-device");
    const outputSelect = document.querySelector("#output-device");

    if (!inputSelect || !inputSelect.value) {
      window.Utils.showNotification("No input device selected. Please select an input device.", "error");
      return;
    }
    if (!outputSelect || !outputSelect.value) {
      window.Utils.showNotification("No output device selected. Please select an output device.", "error");
      return;
    }

    try {
      const outputDevice = outputSelect.value;
      window.Utils.showNotification(`Switching audio to:\nInput: ${inputSelect.value}\nOutput: ${outputDevice}`, "info");
      await window.TauriBridge.startAudioCapture(inputSelect.value, outputDevice);
window.Utils.showNotification(`Audio capture started\nInput: ${inputSelect.value}\nOutput: ${outputDevice}`, "success");
      } catch (error) {
        console.error("Failed to start audio capture:", error);
        window.Utils.showNotification(`Failed to start audio capture: ${window.Utils.sanitizeErrorMessage(error)}`, "error");
    }
  };

  /**
   * Initialize app based on user name
   */
  const init = () => {
    const user = getUserName();
    setProfileNameField(user);

    const promptEl = document.querySelector("#user-name-prompt");
    const mainEl = document.querySelector("#main-content");
    const userNameEl = document.querySelector("#user-name");

    if (user) {
      if (promptEl) promptEl.style.display = "none";
      if (mainEl) mainEl.style.display = "";
      if (userNameEl) userNameEl.textContent = user;

      populateAudioInputDevices();
      populateAudioOutputDevices();
      if (window.Audio3D) {
        window.Audio3D.initBandCanvas();
        window.Audio3D.initializeSpatialCanvas();
      }
    } else {
      if (promptEl) {
        promptEl.classList.remove('user-name-prompt-hidden');
        promptEl.classList.add('visible');
      }
      if (mainEl) mainEl.style.display = "none";
    }
  };

  /**
   * Start visualizer animation loop
   */
  const startVisualizerLoop = () => {
    // Intentionally empty. Visualizer is event-driven now.
  };

  /**
   * Stop visualizer animation loop
   */
  const stopVisualizerLoop = () => {
    // Intentionally empty.
  };

  /**
   * Setup event delegation
   */
  const setupEventDelegation = () => {
    document.addEventListener('click', (e) => {
      const target = e.target;

      if (target.matches('.open-btn') || target.closest('.open-btn')) {
        e.preventDefault();
        openSideMenu();
      } else if (target.matches('.close-btn') || target.closest('.close-btn')) {
        e.preventDefault();
        closeSideMenu();
      } else if (target.matches('#beamng-link') || target.closest('#beamng-link')) {
        e.preventDefault();
        closeSideMenu();
        scrollToBeamNgPanel();
      } else if (target.matches('#eq-link') || target.closest('#eq-link')) {
        e.preventDefault();
        closeSideMenu();
        scrollToEQPanel();
      } else if (target.matches('#settings-link') || target.closest('#settings-link')) {
        e.preventDefault();
        closeSideMenu();
        scrollToSettingsPanel();
      } else if (target.matches('#log-link') || target.closest('#log-link')) {
        e.preventDefault();
        closeSideMenu();
        scrollToLogPanel();
      } else if (target.matches('#session-stats-link') || target.closest('#session-stats-link')) {
        e.preventDefault();
        closeSideMenu();
        scrollToSessionStatsPanel();
      } else if (target.matches('#amp-link') || target.closest('#amp-link')) {
        e.preventDefault();
        closeSideMenu();
        scrollToAmpPanel();
      } else if (target.matches('#reset-eq') || target.closest('#reset-eq')) {
        e.preventDefault();
        resetEQ();
      } else if (target.matches('#save-preset') || target.closest('#save-preset')) {
        e.preventDefault();
        const input = document.querySelector("#preset-name-input");
        if (input) saveEqPreset(input.value);
      } else if (target.matches('#load-preset') || target.closest('#load-preset')) {
        e.preventDefault();
        const select = document.querySelector("#preset-select");
        if (select) loadEqPreset(select.value);
      } else if (target.matches('#delete-preset') || target.closest('#delete-preset')) {
        e.preventDefault();
        const select = document.querySelector("#preset-select");
        if (select) deleteEqPreset(select.value);
      } else if (target.matches('#upload-preset') || target.closest('#upload-preset')) {
        e.preventDefault();
        const select = document.querySelector("#preset-select");
        const input = document.querySelector("#preset-name-input");
        let presetName = null;
        if (select && select.value) {
          presetName = select.value;
        } else if (input && input.value.trim()) {
          presetName = input.value.trim();
        }
        if (presetName) {
          uploadEqPresetToDiscord(presetName);
        } else {
          window.Utils.showNotification("Please enter a preset name or select an existing preset to upload", "warning");
        }
      } else if (target.matches('#reset-settings') || target.closest('#reset-settings')) {
        e.preventDefault();
        resetSettingsControls();
        window.Utils.showNotification("Settings reset to defaults", "info");
      } else if (target.matches('#save-settings') || target.closest('#save-settings')) {
        e.preventDefault();
        saveAppConfig();
      } else if (target.matches('#start-session') || target.closest('#start-session')) {
        e.preventDefault();
        scrollToBeamNgPanel();
      } else if (target.matches('#save-discord-webhook') || target.closest('#save-discord-webhook')) {
        e.preventDefault();
        const webhookInput = document.querySelector("#discord-webhook-url");
        if (webhookInput) {
          const url = webhookInput.value.trim() || null;
          if (url && !window.Utils.validateDiscordWebhookUrl(url)) {
            window.Utils.showNotification('Invalid Discord webhook URL. Must be a valid HTTPS URL from discord.com or discordapp.com', 'error');
            return;
          }
          window.TauriBridge.setDiscordWebhookUrl(url)
            .then(() => {
              window.Utils.showNotification("Discord webhook URL saved successfully!", "success");
            })
            .catch(error => {
              console.error("Failed to save webhook:", error);
              window.Utils.showNotification(`Failed to save webhook: ${window.Utils.sanitizeErrorMessage(error)}`, "error");
            });
        }
      } else if (target.matches('#browse-beamng-mods') || target.closest('#browse-beamng-mods')) {
        e.preventDefault();
        window.TauriBridge.browseBeamNgMods();
      } else if (target.matches('#save-beamng-mods-folder') || target.closest('#save-beamng-mods-folder')) {
        e.preventDefault();
        window.TauriBridge.saveBeamNgModsFolder();
      } else if (target.matches('#check-mod-status') || target.closest('#check-mod-status')) {
        e.preventDefault();
        window.TauriBridge.checkModStatus();
      }
    });

    // User name form
    const userNameForm = document.querySelector("#user-name-form");
    if (userNameForm) {
      userNameForm.addEventListener("submit", (e) => {
        e.preventDefault();
        const input = document.querySelector("#user-name-input");
        const newName = input ? input.value.trim() : "";
        if (newName) {
          localStorage.setItem(userNameKey, newName);
          init();
          window.Utils.showNotification(`Profile name updated to "${newName}"`, "success");
        }
      });
    }

    // Theme color picker
    const themeColorInput = document.getElementById("theme-color-input");
    if (themeColorInput) {
      themeColorInput.addEventListener("input", (e) => {
        saveThemeColor(e.target.value);
      });
    }

    const themePresets = document.querySelectorAll(".theme-preset");
    themePresets.forEach(preset => {
      preset.addEventListener("click", () => {
        const color = preset.dataset.color;
        saveThemeColor(color);
        if (themeColorInput) themeColorInput.value = color;
        window.Utils.showNotification(`Theme color changed`, "success");
      });
    });

    const resetThemeBtn = document.getElementById("reset-theme-color");
    if (resetThemeBtn) {
      resetThemeBtn.addEventListener("click", resetThemeColor);
    }

    // Change handlers
    document.addEventListener('change', (e) => {
      // Fix #1 Bug #6: Only save config on device change, don't restart audio stream
      // Audio restart should only happen from explicit button click
      if (e.target.matches('#input-device')) {
        saveAppConfig();
      }
      if (e.target.matches('#output-device')) {
        saveAppConfig();
      }
      if (e.target.matches('.eq-slider')) {
        const bandIndex = parseInt(e.target.dataset.band, 10);
        updateEQValue(bandIndex, e.target.value);
      }
      if (e.target.matches('#left-gain')) {
        updateChannelGain('left', e.target.value);
      }
      if (e.target.matches('#right-gain')) {
        updateChannelGain('right', e.target.value);
      }
    });
  };

  /**
   * Setup Tauri event listeners
   */
  const setupTauriListeners = async () => {
    try {
      // Rust debug log listener - shows Rust eprintln! in browser console and UI
      // Throttled to once every 3-5 seconds to avoid spam
      await window.TauriBridge.listen("rust_log", (event) => {
        window.Utils.throttleLog('rust_log', console.log, ['[Rust Log]', event.payload], 3000, 5000);
        window.Utils.logToUI('log', `[Rust] ${event.payload}`);
      });

      // Rust debug_log listener - for UDP listener startup messages
      await window.TauriBridge.listen("debug_log", (event) => {
        console.log('[Debug Log]', event.payload);
        window.Utils.logToUI('info', `[Debug] ${event.payload}`);
      });

      // Individual band events from Rust FFT processing (throttled to every 3-5 seconds)
      await window.TauriBridge.listen("band_subwoofer", (event) => {
        const value = event.payload;
        window.Utils.throttleLog('band_subwoofer', console.log, ['[Tauri Event] band_subwoofer:', value], 3000, 5000);
        if (window.Audio3D) {
          window.Audio3D.drawBandWaveform('subwoofer', typeof value === 'number' ? value : 0);
        }
      });

      await window.TauriBridge.listen("band_bass", (event) => {
        const value = event.payload;
        if (window.Audio3D) {
          window.Audio3D.drawBandWaveform('bass', typeof value === 'number' ? value : 0);
        }
      });

      await window.TauriBridge.listen("band_mids", (event) => {
        const value = event.payload;
        if (window.Audio3D) {
          window.Audio3D.drawBandWaveform('mids', typeof value === 'number' ? value : 0);
        }
      });

      await window.TauriBridge.listen("band_treble", (event) => {
        const value = event.payload;
        if (window.Audio3D) {
          window.Audio3D.drawBandWaveform('treble', typeof value === 'number' ? value : 0);
        }
      });

      // BeamNG telemetry for speaker indicators
      // Fix #1 Bug #9: Use handleBeamngTelemetry to also update internal telemetry state
      await window.TauriBridge.listen("beamng_telemetry", (event) => {
        if (window.Audio3D) {
          window.Audio3D.handleBeamngTelemetry(event);
        }
      });

      // BeamNG camera data
      await window.TauriBridge.listen("beamng_camera_data", (event) => {
        const data = event.payload;
        if (window.Audio3D) {
          window.Audio3D.handleBeamngCameraData(data);
        }
      });

      // Electrics UDP values updated
      await window.TauriBridge.listen("electrics_values_updated", (event) => {
        const data = event.payload;
        if (electricsState.udpEnabled && data.eq_bands && data.volume !== undefined) {
          electricsState.eqBandValues = data.eq_bands;
          electricsState.volumeValue = data.volume;
          updateEqFromElectrics(data.eq_bands, data.volume);
        }
      });

      await window.TauriBridge.listen("media_info", (event) => {
        updateMediaInfo(event.payload);
      });

      console.log("[Tauri] Event listeners registered");
      window.Utils.logToUI('info', 'Event listeners registered');
    } catch (error) {
      console.error("[Tauri] Failed to setup event listeners:", error);
      window.Utils.logToUI('error', 'Failed to setup event listeners: ' + error);
    }
  };

  /**
   * Update media info display
   */
  const updateMediaInfo = (mediaInfo) => {
    const updateDOM = () => {
      const titleEl = document.getElementById("media-title");
      const artistEl = document.getElementById("media-artist");
      const durationEl = document.getElementById("media-duration");
      const statusEl = document.getElementById("media-status");
      const thumbnailEl = document.getElementById("media-thumbnail");
      const iconEl = document.getElementById("media-icon");

      if (titleEl) titleEl.textContent = mediaInfo.title || "No Media";
      if (artistEl) artistEl.textContent = mediaInfo.artist || "";
      if (durationEl) {
        if (mediaInfo.total_time > 0) {
          const current = window.Utils.formatTime(mediaInfo.current_time || 0);
          const total = window.Utils.formatTime(mediaInfo.total_time);
          durationEl.textContent = `${current} / ${total}`;
        } else if (mediaInfo.current_time > 0) {
          durationEl.textContent = window.Utils.formatTime(mediaInfo.current_time);
        } else {
          durationEl.textContent = "";
        }
      }
      if (statusEl) {
        statusEl.textContent = mediaInfo.is_playing ? "Now Playing" : "Paused";
      }
      lastMediaIsPlaying = mediaInfo.is_playing;

      const playPauseBtn = document.getElementById("media-play-pause-btn");
      if (playPauseBtn) {
        playPauseBtn.innerHTML = mediaInfo.is_playing ? "&#9208;" : "&#9654;";
      }

      if (thumbnailEl && iconEl) {
        if (mediaInfo.thumbnail) {
          thumbnailEl.src = mediaInfo.thumbnail;
          thumbnailEl.style.display = "block";
          iconEl.style.display = "none";
        } else {
          thumbnailEl.style.display = "none";
          iconEl.style.display = "flex";
        }
      }
    };

    if ('requestIdleCallback' in window) {
      requestIdleCallback(updateDOM, { timeout: 100 });
    } else {
      setTimeout(updateDOM, 0);
    }
  };

  /**
   * Setup media transport control buttons (prev / play-pause / next)
   */
  const setupMediaControls = () => {
    const prevBtn = document.getElementById("media-prev-btn");
    const playPauseBtn = document.getElementById("media-play-pause-btn");
    const nextBtn = document.getElementById("media-next-btn");

    const sendCommand = (command, label) => {
      if (!window.TauriBridge || typeof command !== "function") return;
      command
        .call(window.TauriBridge)
        .then(() => window.TauriBridge.getMediaInfo())
        .then((info) => updateMediaInfo(info))
        .catch((err) => {
          console.error(`[UI] Media ${label} failed:`, err);
          window.Utils.showNotification(
            `Media ${label.replace("_", " ")} failed: ${err}`,
            "error"
          );
          window.Utils.throttleLog(
            `media_${label}_err`,
            window.Utils.logToUI,
            ["error", `Media ${label} failed: ${err}`],
            10000,
            15000
          );
        });
    };

    if (prevBtn) {
      prevBtn.addEventListener("click", () => sendCommand(window.TauriBridge.mediaPrevious, "previous"));
    }
    if (playPauseBtn) {
      playPauseBtn.addEventListener("click", () => sendCommand(window.TauriBridge.mediaPlayPause, "play_pause"));
    }
    if (nextBtn) {
      nextBtn.addEventListener("click", () => sendCommand(window.TauriBridge.mediaNext, "next"));
    }
  };

  /**
   * Setup 3D audio listeners
   */
  const setup3dAudioListeners = () => {
    // PC Audio Capture button
    const audioCaptureBtn = document.getElementById('start-audio-capture-btn');
    const audioCaptureStatus = document.getElementById('audio-capture-status');
    if (audioCaptureBtn) {
      audioCaptureBtn.addEventListener('click', async () => {
        const isCapturing = audioCaptureBtn.dataset.capturing === 'true';
        if (isCapturing) {
          try {
            await window.TauriBridge.stopAudioCapture();
            audioCaptureBtn.textContent = 'Start PC Audio Capture';
            audioCaptureBtn.dataset.capturing = 'false';
            if (audioCaptureStatus) {
              audioCaptureStatus.textContent = 'Not capturing';
            }
            window.Utils.showNotification('Audio capture stopped', 'info');
          } catch (err) {
            console.error('[Audio Capture] Failed to stop:', err);
            window.Utils.logToUI('error', 'Failed to stop audio: ' + window.Utils.sanitizeErrorMessage(err));
            window.Utils.showNotification('Failed to stop audio capture: ' + err, 'error');
          }
        } else {
          const inputSelect = document.getElementById('input-device');
          const outputSelect = document.getElementById('output-device');
          const inputDevice = inputSelect ? inputSelect.value : '';
          const outputDevice = outputSelect ? outputSelect.value : '';
          
          console.log('[Audio Capture] Starting with input:', inputDevice, 'output:', outputDevice);
          window.Utils.logToUI('info', `Starting audio: ${inputDevice} → ${outputDevice}`);
          
          if (!inputDevice) {
            window.Utils.showNotification('Please select an input device first', 'error');
            return;
          }
          if (!outputDevice) {
            window.Utils.showNotification('Please select an output device first', 'error');
            return;
          }
          
          try {
            await window.TauriBridge.startAudioCapture(inputDevice, outputDevice);
            audioCaptureBtn.textContent = 'Stop PC Audio Capture';
            audioCaptureBtn.dataset.capturing = 'true';
            if (audioCaptureStatus) {
              audioCaptureStatus.textContent = 'Capturing - Visualizers should be moving';
            }
            window.Utils.showNotification('Audio capture started! Check console for device info', 'success');
          } catch (err) {
            console.error('[Audio Capture] Failed to start:', err);
            window.Utils.logToUI('error', 'Failed to start audio: ' + window.Utils.sanitizeErrorMessage(err));
            window.Utils.showNotification('Failed to start: ' + err, 'error');
          }
        }
      });
      audioCaptureBtn.dataset.capturing = 'false';
    }
  };

  /**
   * Setup chat listeners
   */
  const setupChatListeners = () => {
    const chatInput = document.getElementById('chat-input');
    if (chatInput) {
      chatInput.addEventListener('keypress', (e) => {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault();
          if (window.ChatSystem) window.ChatSystem.sendMessage();
        }
      });
    }

    const addServerBtn = document.querySelector('.add-server');
    if (addServerBtn) {
      addServerBtn.addEventListener('click', () => {
        if (window.ChatSystem) window.ChatSystem.showCreateServerModal();
      });
    }

    const giftBtn = document.getElementById('chat-gift-btn');
    if (giftBtn) {
      giftBtn.addEventListener('click', () => {
        if (window.ChatSystem) window.ChatSystem.showShareEqPresetModal();
      });
    }

    const membersBtn = document.getElementById('chat-members-btn');
    if (membersBtn) {
      membersBtn.addEventListener('click', () => {
        if (window.ChatSystem) window.ChatSystem.toggleMembersSidebar();
      });
    }

    // Modal buttons
    document.getElementById('create-server-cancel')?.addEventListener('click', () => {
      if (window.ChatSystem) window.ChatSystem.hideCreateServerModal();
    });
    document.getElementById('share-eq-cancel')?.addEventListener('click', () => {
      if (window.ChatSystem) window.ChatSystem.hideShareEqPresetModal();
    });
    document.getElementById('create-server-confirm')?.addEventListener('click', () => {
      if (window.ChatSystem) window.ChatSystem.createServer();
    });
    document.getElementById('share-eq-confirm')?.addEventListener('click', () => {
      if (window.ChatSystem) window.ChatSystem.shareEqPreset();
    });

    // Channel selection
    document.querySelectorAll('.channel-item').forEach(item => {
      item.addEventListener('click', () => {
        const channelId = item.dataset.channelId;
        const channelName = item.querySelector('.channel-name')?.textContent || channelId;
        if (window.ChatSystem) window.ChatSystem.selectChannel(channelId, channelName);
      });
    });

    // Home server click
    document.querySelector('.home-server')?.addEventListener('click', () => {
      if (window.ChatSystem) window.ChatSystem.selectServer('home');
    });

    // Electrics UDP toggle
    const electricsToggle = document.getElementById('electrics-udp-toggle');
    if (electricsToggle) {
      electricsToggle.addEventListener('change', async (e) => {
        await toggleElectricsUdp(e.target.checked);
      });
    }

    // Visualizer transport toggle (HTTP polling vs WebSocket push)
    const visualizerWsToggle = document.getElementById('visualizer-ws-toggle');
    if (visualizerWsToggle) {
      const savedWsMode = localStorage.getItem('visualizerWsMode') === 'true';
      visualizerWsToggle.checked = savedWsMode;
      if (savedWsMode) {
        window.TauriBridge.setVisualizerWsMode(true).catch((error) => {
          console.error('[UI] Failed to restore WebSocket visualizer mode:', error);
        });
      }
      visualizerWsToggle.addEventListener('change', async (e) => {
        try {
          await window.TauriBridge.setVisualizerWsMode(e.target.checked);
          localStorage.setItem('visualizerWsMode', String(e.target.checked));
          window.Utils.logToUI('info', e.target.checked
            ? 'Visualizer transport: WebSocket (ws://0.0.0.0:5001, values pushed ~30 fps)'
            : 'Visualizer transport: HTTP (http://0.0.0.0:5000/values, polling)');
        } catch (error) {
          console.error('[UI] Failed to switch visualizer transport:', error);
          window.Utils.showNotification('Failed to switch visualizer transport: ' + window.Utils.sanitizeErrorMessage(error), 'error');
          visualizerWsToggle.checked = !e.target.checked;
        }
      });
    }
  };

  /**
   * Cleanup on unload
   */
  const setupCleanupHandler = () => {
    window.addEventListener('beforeunload', () => {
      console.log('[Cleanup] Page unloading - cleaning up resources...');
      window.Utils.logToUI('warn', 'Page unloading - cleaning up');

      if (mediaInfoInterval) clearInterval(mediaInfoInterval);
      if (cameraPollInterval) clearInterval(cameraPollInterval);

      stopVisualizerLoop();

      console.log('[Cleanup] Done - all resources released');
      window.Utils.logToUI('info', 'Cleanup complete');
    });
  };

  /**
   * Main initialization
   */
  const initializeApp = async () => {
    // Initialize log panel first
    window.Utils.initLogPanel();
    window.Utils.logToUI('info', 'Application starting...');

    // Load theme color
    loadThemeColor();

    // Initialize app
    init();

    // Hide loading screen after 1500ms regardless of what happens below
    setTimeout(() => {
      const loadingScreen = document.getElementById('loading-screen');
      if (loadingScreen) {
        loadingScreen.classList.add('hidden');
        document.body.classList.add('loaded');
        window.Utils.logToUI('info', 'App loaded successfully');
      }
    }, 1500);

    // Load app config
    await loadAppConfig();

    // Load preset names
    await loadPresetNames();

    // Setup event delegation
    setupEventDelegation();

    // Setup media transport controls
    setupMediaControls();

    // Setup Tauri event listeners
    await setupTauriListeners();

    // Setup feature listeners
    setup3dAudioListeners();
    setupChatListeners();

    // Setup cleanup handler
    setupCleanupHandler();

    // Start visualizer animation loop
    startVisualizerLoop();

    // Start media info polling (fallback, push events are primary)
    mediaInfoInterval = setInterval(() => {
      if (!lastMediaIsPlaying) return; // Skip if not playing, wait for push event
      window.TauriBridge.getMediaInfo()
        .then(info => updateMediaInfo(info))
        .catch(err => {
          console.error("Failed to fetch media info:", err);
          // Only log errors every 10 seconds for polling operations
          window.Utils.throttleLog('media_info_poll', console.error, ['Media info poll failed:', err], 10000, 15000);
        });
    }, 30000);

    // Initial media info fetch
    window.TauriBridge.getMediaInfo()
      .then(info => updateMediaInfo(info))
      .catch(err => console.error("Failed to fetch initial media info:", err));

    // Poll camera data fallback (controlled by 3d-audio.js now)
    // Removed inline setInterval

    // (loading screen timeout moved to top of initializeApp)

    console.log('[UI] App initialized successfully');
    window.Utils.logToUI('info', 'App initialized');
  };

  // Wait for DOM and all modules to be ready
  document.addEventListener('DOMContentLoaded', function() {
    // Small delay to ensure all modules are loaded
    setTimeout(initializeApp, 100);
  });

  // Expose UI module globally
  window.UI = {
    applyThemeColor,
    saveThemeColor,
    loadThemeColor,
    resetThemeColor,
    getUserName,
    setProfileNameField,
    openSideMenu,
    closeSideMenu,
    scrollToBeamNgPanel,
    scrollToEQPanel,
    scrollToSettingsPanel,
    scrollToLogPanel,
    scrollToAmpPanel,
    resetSettingsControls,
    updateEQValue,
    resetEQ,
    saveEqPreset,
    loadEqPreset,
    deleteEqPreset,
    uploadEqPresetToDiscord,
    loadPresetNames,
    updateChannelGain,
    saveAppConfig,
    loadAppConfig,
    populateAudioInputDevices,
    populateAudioOutputDevices,
    startAudioCapture,
    updateMediaInfo,
    toggleElectricsUdp,
    startCameraPolling,
    stopCameraPolling
  };

  console.log('[UI] Module loaded, waiting for DOM...');
  window.Utils.logToUI('info', 'UI module loaded');
})();
