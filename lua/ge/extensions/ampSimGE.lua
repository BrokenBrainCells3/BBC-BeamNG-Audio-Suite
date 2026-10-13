-- ampSimGE - sends amplifier/subwoofer/enclosure simulation state to the app
--
-- Implements the AMP_SIMULATION.md telemetry packet: one flat
-- {"command": "amp_state", ...} JSON every ~10 Hz containing every value the
-- app needs to simulate the amplifier chain, the subwoofer drivers and the
-- enclosures.
--
-- Dynamic values come from the player vehicle's electrics via
-- core_vehicleBridge.getCachedVehicleData (same mechanism mediaInfoGE uses):
--   supply_volts <- electrics.values.volts   (voltageSim.lua, both controllers)
--   rpm          <- electrics.values.rpm
--   sub/bass/mid/treble <- electrics.values.*
-- Everything else is config (defaults below) plus small thermal/damage models
-- that run here: amp temp, voice coil temp + power compression, sub break-in,
-- air leak growth on hard clipping, blown fuse on overcurrent.
--
-- Transport: prefers the open websocket via subwooferWsGE.send() when that
-- extension is loaded and connected; falls back to the UDP command channel
-- (127.0.0.1:5001) used by mediaInfoGE/mediaBridgeGE. The app treats both as
-- amp_state packets.
--
-- Usage: extensions.load('ampSimGE')  (auto-loaded by modscript.lua)
--   ampSimGE.setConfig{ rms_power_w = 3000, impedance_ohm = 0.5, ... }
--   ampSimGE.setRemoteLevel(0.7)      -- remote bass knob 0..1
--   ampSimGE.replaceFuse()
--   ampSimGE.sendNow()                -- one packet immediately
--   ampSimGE.getStatus()

local M = {}

local json = dofile("lua/vehicle/controller/dkjson.lua")

-- Manual override lock (setConfig blocks the vehicle's jbeam re-push for a
-- test window, then auto-resumes). Declared HERE at the top because onUpdate
-- uses them; the Public API section below only assigns.
local jbeamPushEnabled = true
local pushLockExpiry = 0 -- os.clock() timestamp when the lock auto-lifts
local PUSH_LOCK_SECONDS = 10

-- math.log(x, 10) is not available on LuaJIT 5.1
local log10 = math.log10 or function(x) return math.log(x) / math.log(10) end

local SEND_INTERVAL = 1 / 30 -- 30 Hz (snappy for live tuning; ~60 KB/s over UDP)
local AMBIENT_C = 25

-- Thermal/damage model constants
local AMP_HEAT_GAIN = 0.004   -- dT/s per W of lost power
local AMP_COOL_K = 0.012      -- dT/s per K above ambient (scaled by airflow)
local COIL_HEAT_GAIN = 0.02   -- dT/s per W per sub
local COIL_COOL_K = 0.008
local CU_TEMP_COEF = 0.00393  -- copper resistance temperature coefficient
local AMP_PROTECT_C = 95
local COIL_MAX_C = 250

-- Static config: mirrors the AMP_SIMULATION.md defaults. Override any field
-- with ampSimGE.setConfig{...}
local cfg = {
  -- amp
  gain_db = 24.0,
  preout_v = 4.0,
  bass_boost_db = 12.0,
  bass_boost_freq_hz = 45,
  subsonic_hz = 24,
  subsonic_slope_db_oct = 24,
  lpf_hz = 80,
  lpf_slope_db_oct = 24,
  phase_deg = 0,
  rms_power_w = 1500,
  impedance_ohm = 1.0,
  bridged = true,
  channel_config = "mono",
  amp_class = "D",
  efficiency = 0.8,
  damping_factor = 150,
  fuse_a = 150,
  -- sub drivers
  sub_count = 2,
  sub_size_in = 12,
  sub_sensitivity_db = 88,
  sub_rms_w = 1000,
  sub_peak_w = 2000,
  sub_nominal_ohm = 2,
  vc_config = "DVC 2ohm",
  vc_wiring = "parallel",
  sd_cm2 = 480,
  xmax_mm = 16,
  fs_hz = 32,
  qts = 0.42,
  vas_l = 45,
  qes = 0.5,
  qms = 5.0,
  vc_inch = 3,
  vc_wire = "cu",
  vc_former = "alu",
  bl_tm = 22,
  magnet_oz = 250,
  cone_mat = "paper",
  surround_mat = "foam",
  spider_mat = "nomex",
  suspension_stiffness = 1.0,
  -- enclosure
  enclosure_type = "ported",
  enclosure_l = 85,
  port_tune_hz = 34,
  port_area_cm2 = 96,
  port_len_cm = 55,
  port_shape = "slot",
  enclosure_mat = "mdf",
  enclosure_thick_mm = 19,
  bracing_pct = 20,
  polyfill_pct = 0,
  sub_facing = "back",
  cabin_gain_db = 6,
  -- model tuning
  breakin_hours = 40, -- playing hours from 0% to 100% broken in
}

-- Dynamic state
local remoteLevel = 1.0 -- remote bass knob
local ampTemp = AMBIENT_C
local coilTemp = AMBIENT_C
local playHours = 0
local leakPct = 0
local fuseBlown = false
local thermalProtect = false

-- Cached from the vehicle
local supplyVolts = 14.4
local rpm = 0
local subLevel = 0
local bandBass, bandMid, bandTreble = 0, 0, 0
local mediaPlaying = false

-- Send bookkeeping
local sendTimer = 0
local lastClip = false
local lastClipOver = 0
local packetsSent = 0

local function dlog(level, msg)
  log(level, "ampSimGE", tostring(msg))
end

-- UDP fallback channel (same socket pattern as mediaInfoGE/mediaBridgeGE)
local hasSocket = false
local udp = nil
do
  local ok, socketModule = pcall(require, "socket")
  if ok then
    hasSocket = true
    udp = socketModule.udp()
    udp:settimeout(0)
    udp:setpeername("127.0.0.1", 5001)
  end
end

local function wsAvailable()
  return subwooferWsGE and subwooferWsGE.getStatus and subwooferWsGE.getStatus().connected
end

-- Transport: UDP 5001 FIRST - the same socket/pattern as the keyboard and
-- media command relay in mediaBridgeGE (proven end-to-end). WebSocket via
-- subwooferWsGE.send() is only the fallback when no socket is available.
local function sendPacket(payload)
  local ok, encoded = pcall(json.encode, payload)
  if not ok or not encoded then return false end
  if hasSocket and udp then
    local success = udp:send(encoded)
    if success then return true end
    dlog("W", "udp send failed, falling back to websocket")
  end
  if wsAvailable() then
    return subwooferWsGE.send(encoded)
  end
  return false
end

local function readVehicle()
  local veh = be and be.getPlayerVehicle and be:getPlayerVehicle(0) or nil
  if not veh then return end
  local vehId = veh:getId()
  local volts = core_vehicleBridge.getCachedVehicleData(vehId, "volts")
  if volts then supplyVolts = volts end
  local r = core_vehicleBridge.getCachedVehicleData(vehId, "rpm")
  if r then rpm = r end
  local s = core_vehicleBridge.getCachedVehicleData(vehId, "subWoofer")
  if s then subLevel = s end
  local b = core_vehicleBridge.getCachedVehicleData(vehId, "bass")
  if b then bandBass = b end
  local m = core_vehicleBridge.getCachedVehicleData(vehId, "mid")
  if m then bandMid = m end
  local t = core_vehicleBridge.getCachedVehicleData(vehId, "treble")
  if t then bandTreble = t end
  local p = core_vehicleBridge.getCachedVehicleData(vehId, "mediaIsPlaying")
  if p ~= nil then mediaPlaying = p end
end

local function buildPacket(dt)
  local engineOn = (rpm or 0) > 200
  local headroomFrac = (supplyVolts / 14.4) ^ 2

  -- Clipping: sub demand (with remote knob + boost) vs supply headroom
  local demand = (subLevel or 0) * (cfg.remote_level or remoteLevel)
  local clip = demand > headroomFrac and not fuseBlown
  local clipOver = 0
  if clip then
    clipOver = 20 * log10(math.max(demand, 0.001) / math.max(headroomFrac, 0.001))
    if clipOver > 6 then clipOver = 6 end
    -- hard clipping slowly leaks the box
    leakPct = math.min(100, leakPct + dt * 0.001 * clipOver)
  end
  lastClip = clip
  lastClipOver = clipOver

  -- Power model
  local demandCapped = math.min(demand, 1)
  local pOut = cfg.rms_power_w * demandCapped * headroomFrac
  if fuseBlown then pOut = 0 end
  if thermalProtect then pOut = pOut * 0.3 end
  local airflow = 1 + (rpm or 0) / 3000

  -- Amp thermal: lost power heats, airflow cools; protect reduces output
  local pLoss = pOut * (1 / math.max(cfg.efficiency, 0.1) - 1)
  ampTemp = ampTemp + (pLoss * AMP_HEAT_GAIN - (ampTemp - AMBIENT_C) * AMP_COOL_K * airflow) * dt
  if ampTemp < AMBIENT_C then ampTemp = AMBIENT_C end
  thermalProtect = ampTemp >= AMP_PROTECT_C

  -- Voice coil thermal + power compression (copper Re rises with temp)
  local pPerSub = pOut / math.max(cfg.sub_count, 1)
  coilTemp = coilTemp + (pPerSub * COIL_HEAT_GAIN - (coilTemp - AMBIENT_C) * COIL_COOL_K * airflow) * dt
  if coilTemp < AMBIENT_C then coilTemp = AMBIENT_C end
  if coilTemp > COIL_MAX_C then coilTemp = COIL_MAX_C end
  local reFactor = 1 + CU_TEMP_COEF * (coilTemp - 20)
  local compressionDb = -10 * log10(reFactor) -- 0 .. negative dB

  -- Break-in: stiffness relaxes over play time
  if mediaPlaying and demandCapped > 0.05 then
    playHours = playHours + dt / 3600
  end
  local breakinPct = math.min(100, (playHours / math.max(cfg.breakin_hours, 0.1)) * 100)

  -- Fuse: overcurrent blows it
  local current = pOut / math.max(cfg.efficiency, 0.1) / math.max(supplyVolts, 6)
  if not fuseBlown and current > cfg.fuse_a then
    fuseBlown = true
    dlog("W", string.format("fuse blown: %.1f A > %.0f A rating", current, cfg.fuse_a))
  end

  local pkt = {
    command = "amp_state",
    -- input / gain stage
    gain_db = cfg.gain_db,
    preout_v = cfg.preout_v,
    remote_level = cfg.remote_level or remoteLevel,
    bass_boost_db = cfg.bass_boost_db,
    bass_boost_freq_hz = cfg.bass_boost_freq_hz,
    -- filters
    subsonic_hz = cfg.subsonic_hz,
    subsonic_slope_db_oct = cfg.subsonic_slope_db_oct,
    lpf_hz = cfg.lpf_hz,
    lpf_slope_db_oct = cfg.lpf_slope_db_oct,
    phase_deg = cfg.phase_deg,
    -- amp hardware
    rms_power_w = cfg.rms_power_w,
    impedance_ohm = cfg.impedance_ohm,
    bridged = cfg.bridged,
    channel_config = cfg.channel_config,
    amp_class = cfg.amp_class,
    efficiency = cfg.efficiency,
    damping_factor = cfg.damping_factor,
    fuse_a = cfg.fuse_a,
    -- model state fields that live tuning can force: when cfg carries an
    -- explicit value (electrics/tuner override), it wins over the model
    fuse_blown = (cfg.fuse_blown ~= nil) and cfg.fuse_blown or fuseBlown,
    -- supply / thermal
    supply_volts = supplyVolts,
    rpm = rpm,
    engine_on = engineOn,
    amp_temp_c = math.floor(ampTemp * 10) / 10,
    thermal_protect = (cfg.thermal_protect ~= nil) and cfg.thermal_protect or thermalProtect,
    -- signal
    sub_level = subLevel,
    bass = bandBass,
    mid = bandMid,
    treble = bandTreble,
    clip = clip,
    clip_db_over = math.floor(clipOver * 100) / 100,
    -- sub drivers
    sub_count = cfg.sub_count,
    sub_size_in = cfg.sub_size_in,
    sub_sensitivity_db = cfg.sub_sensitivity_db,
    sub_rms_w = cfg.sub_rms_w,
    sub_peak_w = cfg.sub_peak_w,
    sub_nominal_ohm = cfg.sub_nominal_ohm,
    vc_config = cfg.vc_config,
    vc_wiring = cfg.vc_wiring,
    sd_cm2 = cfg.sd_cm2,
    xmax_mm = cfg.xmax_mm,
    fs_hz = cfg.fs_hz,
    qts = cfg.qts,
    vas_l = cfg.vas_l,
    qes = cfg.qes,
    qms = cfg.qms,
    vc_inch = cfg.vc_inch,
    vc_wire = cfg.vc_wire,
    vc_former = cfg.vc_former,
    bl_tm = cfg.bl_tm,
    magnet_oz = cfg.magnet_oz,
    cone_mat = cfg.cone_mat,
    surround_mat = cfg.surround_mat,
    spider_mat = cfg.spider_mat,
    suspension_stiffness = cfg.suspension_stiffness,
    breakin_pct = math.floor(breakinPct * 10) / 10,
    coil_temp_c = math.floor(coilTemp * 10) / 10,
    power_compression_db = math.floor(compressionDb * 100) / 100,
    -- enclosure
    enclosure_type = cfg.enclosure_type,
    enclosure_l = cfg.enclosure_l,
    port_tune_hz = cfg.port_tune_hz,
    port_area_cm2 = cfg.port_area_cm2,
    port_len_cm = cfg.port_len_cm,
    port_shape = cfg.port_shape,
    enclosure_mat = cfg.enclosure_mat,
    enclosure_thick_mm = cfg.enclosure_thick_mm,
    bracing_pct = cfg.bracing_pct,
    leak_pct = math.floor(leakPct * 10) / 10,
    polyfill_pct = cfg.polyfill_pct,
    sub_facing = cfg.sub_facing,
    cabin_gain_db = cfg.cabin_gain_db,
  }
  return pkt, current
end

local function onUpdate(dt)
  if not dt or dt <= 0 then return end
  -- auto-lift the manual override lock
  if not jbeamPushEnabled and os.clock() >= pushLockExpiry then
    jbeamPushEnabled = true
    pushLockExpiry = 0
    log("I", "ampSimGE", "jbeam push auto-resumed (override window elapsed)")
  end
  sendTimer = sendTimer + dt
  if sendTimer < SEND_INTERVAL then return end
  sendTimer = sendTimer - SEND_INTERVAL

  readVehicle()
  local pkt = buildPacket(SEND_INTERVAL)
  if sendPacket(pkt) then
    packetsSent = packetsSent + 1
  end
end

-- ==============================
-- Public API
-- ==============================

-- Manual override lock: setConfig (console/UI) wins over the vehicle's
-- 2s jbeam re-push, but ONLY temporarily — it auto-resumes 10 s after the
-- last setConfig so a forgotten console override can never permanently
-- block the vehicle's values (and there's no console dependency to undo it).

-- Merge overrides into the config, e.g. ampSimGE.setConfig{ impedance_ohm = 0.5, sub_count = 4 }
-- Blocks the vehicle's jbeam re-push for 10 s (then auto-resumes), so
-- console overrides are temporary test windows, not permanent locks.
M.setConfig = function(overrides)
  if type(overrides) ~= "table" then return false end
  jbeamPushEnabled = false
  pushLockExpiry = os.clock() + PUSH_LOCK_SECONDS
  for k, v in pairs(overrides) do cfg[k] = v end
  return true
end
M.getConfig = function() return cfg end

-- Resume letting the vehicle's jbeam parts drive the config immediately
M.enableJbeamPush = function()
  jbeamPushEnabled = true
  pushLockExpiry = 0
  dlog("I", "jbeam push re-enabled")
  return true
end
M.isJbeamPushEnabled = function() return jbeamPushEnabled end

-- Receives vehicle-side jbeam config pushed by the bbcAmpConfig controller
-- (obj:queueGameEngineLua). Accepts a JSON string or a Lua table.
local configMergeCount = 0
local pushBlockLogT = 0
M.setConfigJson = function(data)
  if not jbeamPushEnabled then
    -- manual override active (setConfig): ignore vehicle pushes, but make
    -- the block visible in beamng.log (throttled)
    if os.clock() >= (pushBlockLogT or 0) then
      log("W", "ampSimGE", "vehicle config push ignored - manual override active for " ..
        string.format("%.0f", math.max(0, pushLockExpiry - os.clock())) .. "s more")
      pushBlockLogT = os.clock() + 5
    end
    return false
  end
  if type(data) == "string" then
    local ok, decoded = pcall(jsonDecode, data)
    if not ok or type(decoded) ~= "table" then
      dlog("W", "setConfigJson: invalid payload: " .. tostring(decoded))
      return false
    end
    data = decoded
  end
  if type(data) ~= "table" then return false end
  -- drop controller metadata keys if a part forwards them
  data.configType = nil
  data.fileName = nil
  data.name = nil
  data.type = nil
  local n = 0
  for _ in pairs(data) do n = n + 1 end
  configMergeCount = configMergeCount + 1
  if configMergeCount <= 2 then
    log("I", "ampSimGE", "merged vehicle config #" .. configMergeCount .. " (" .. n .. " fields) into amp state")
  end
  -- NOTE: merge DIRECTLY - calling M.setConfig here would arm the manual
  -- override lock and every vehicle push would re-lock the pipeline for
  -- 10 s, ignoring electrics values ~95% of the time.
  for k, v in pairs(data) do cfg[k] = v end
  return true
end
M.getConfigMergeCount = function() return configMergeCount end

-- Remote bass knob (0..1); bind this to an input action or call from UI
M.setRemoteLevel = function(v)
  remoteLevel = math.max(0, math.min(1, tonumber(v) or 1))
  cfg.remote_level = remoteLevel
  return remoteLevel
end
M.getRemoteLevel = function() return remoteLevel end

-- Replaces the blown fuse (same rating)
M.replaceFuse = function()
  fuseBlown = false
  dlog("I", "fuse replaced")
  return true
end

-- One immediate packet (uses fresh vehicle data, no thermal stepping)
M.sendNow = function()
  readVehicle()
  local pkt = buildPacket(SEND_INTERVAL)
  return sendPacket(pkt)
end

M.getStatus = function()
  return {
    packetsSent = packetsSent,
    transport = hasSocket and "udp" or (wsAvailable() and "websocket" or "none"),
    jbeamPush = jbeamPushEnabled,
    supply_volts = supplyVolts,
    rpm = rpm,
    amp_temp_c = ampTemp,
    coil_temp_c = coilTemp,
    thermal_protect = thermalProtect,
    fuse_blown = fuseBlown,
    clip = lastClip,
    clip_db_over = lastClipOver,
    leak_pct = leakPct,
    breakin_pct = playHours > 0 and math.min(100, (playHours / math.max(cfg.breakin_hours, 0.1)) * 100) or 0,
    remote_level = cfg.remote_level or remoteLevel,
  }
end

M.onUpdate = onUpdate

rawset(_G, "ampSimGE", M)
return M