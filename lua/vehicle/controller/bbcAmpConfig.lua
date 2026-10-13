-- bbcAmpConfig.lua
-- Vehicle controller that collects amplifier / subwoofer / enclosure sim
-- config from jbeam parts and pushes it into the GE ampSimGE extension.
--
-- Each part jbeam (amp, sub, enclosure) declares its own controller entry
-- with a UNIQUE name and a configType. All scalar fields are merged into the
-- shared vehicle-side registry and sent to ampSimGE.setConfigJson() once the
-- vehicle finishes loading (first update tick = all controllers inited).
--
-- jbeam format (2-element entry, unique "name" INSIDE the data dict -
-- same pattern as stock advancedCouplerControl parts):
--   "controller": [
--     ["fileName"],
--     ["bbcAmpConfig", {"name": "my_amp_cfg", "configType": "amp",
--       "rms_power_w": 1500, ...}],
--   ]
-- (The positional ["file", "name", "type", data] form is NOT valid under
-- the ["fileName"] header - the jbeam table parser rejects it.)
--
-- configType is just documentation/logging; any scalar field from
-- AMP_SIMULATION.md is accepted from any part. Sub parts may omit
-- impedance_ohm and sd_cm2 - they are computed from the driver config
-- (vc_config/vc_wiring/sub_count and sub_size_in) unless set explicitly.
--
-- ELECTRICS OVERRIDE: if electrics.values carries a value under the same
-- name as any field below (e.g. electrics.values.lpf_hz, written by ANY
-- controller/lua on the vehicle), it overrides the jbeam value in the
-- push to the app. Chain: app slider > electrics > jbeam > default.
-- Booleans accept 0/1 or true/false.
--
-- The registry is also readable by other vehicle controllers:
--   bbcAmpSimValues.rms_power_w etc.
--
-- NOTE: ampSimGE holds ONE config for the app, so the last vehicle spawned
-- with bbcAmpConfig parts wins. That matches the single-app setup.

local M = {}
M.type = "auxiliary"

local json = dofile("lua/vehicle/controller/dkjson.lua")

-- Shared registry across all instances of this controller (vehicle Lua _G,
-- per vehicle). rerequire runs this file once per jbeam entry, so the guard
-- keeps one merged table.
if bbcAmpSimValues == nil then bbcAmpSimValues = {} end

-- effective piston area per size (cm2), used when sd_cm2 is not given
local SD_BY_SIZE = {
  [8] = 210, [10] = 330, [12] = 480, [15] = 780, [18] = 1150,
}

local SKIP_KEYS = {
  fileName = true, name = true, type = true, configType = true,
  manualOrder = true, hasCustomName = true,
}

local instanceName = "?"
local configType = "amp"
local sendTimer = 0

-- final load the amp sees: coils per vc_config/vc_wiring, subs in parallel
local function computeImpedance(cfg)
  local nominal = tonumber(cfg.sub_nominal_ohm)
  if not nominal then return nil end
  local vc = tostring(cfg.vc_config or "SVC")
  local wiring = tostring(cfg.vc_wiring or "parallel")
  local perSub = nominal
  if vc:find("DVC") then
    if wiring == "parallel" then
      perSub = nominal / 2
    elseif wiring == "series" then
      perSub = nominal * 2
    end
    -- "series-parallel" on a single sub = one coil pair combo; keep nominal
  end
  local count = math.max(tonumber(cfg.sub_count) or 1, 1)
  return perSub / count
end

-- apply derived values (only where no part set them explicitly).
-- NOTE: BBC_AMP_IMPEDANCE_EXPLICIT is a SHARED global across all instances
-- of this controller (each jbeam part rerequires this file), otherwise a
-- later instance would recompute an explicitly-set impedance and flap it.
local function finalizeConfig(cfg)
  if not BBC_AMP_IMPEDANCE_EXPLICIT then
    local z = computeImpedance(cfg)
    if z then cfg.impedance_ohm = z end
  end
  if cfg.sd_cm2 == nil and cfg.sub_size_in ~= nil then
    cfg.sd_cm2 = SD_BY_SIZE[tonumber(cfg.sub_size_in)]
  end
end

-- Every amp/sub/box field the app understands. If the vehicle's electrics
-- table carries a value under the SAME name (electrics.values.gain_db etc. -
-- written by any controller, trigger, or custom lua), it overrides the jbeam
-- value sent to the app.
local FIELD_WHITELIST = {
  "gain_db", "preout_v", "remote_level",
  "bass_boost_db", "bass_boost_freq_hz",
  "subsonic_hz", "subsonic_slope_db_oct",
  "lpf_hz", "lpf_slope_db_oct", "phase_deg",
  "rms_power_w", "impedance_ohm", "bridged", "channel_config",
  "amp_class", "efficiency", "damping_factor", "fuse_a",
  "sub_count", "sub_size_in", "sub_sensitivity_db",
  "sub_rms_w", "sub_peak_w", "sub_nominal_ohm",
  "vc_config", "vc_wiring", "sd_cm2", "xmax_mm",
  "fs_hz", "qts", "vas_l", "qes", "qms",
  "vc_inch", "vc_wire", "vc_former", "bl_tm", "magnet_oz",
  "cone_mat", "surround_mat", "spider_mat", "suspension_stiffness",
  "enclosure_type", "enclosure_l", "port_tune_hz",
  "port_area_cm2", "port_len_cm", "port_shape",
  "enclosure_mat", "enclosure_thick_mm", "bracing_pct",
  "polyfill_pct", "sub_facing", "cabin_gain_db",
  "fuse_blown", "thermal_protect",
}
local BOOL_FIELDS = { fuse_blown = true, thermal_protect = true, bridged = true }

-- jbeam registry + electrics.values overrides, as a fresh table per push so
-- a vanished electrics key falls back to the jbeam value automatically.
local function buildPayload()
  local payload = {}
  for k, v in pairs(bbcAmpSimValues) do payload[k] = v end
  local ev = electrics and electrics.values
  if ev then
    for _, k in ipairs(FIELD_WHITELIST) do
      local v = ev[k]
      if v ~= nil then
        if BOOL_FIELDS[k] then
          payload[k] = (v ~= 0 and v ~= false)
        elseif type(v) == "number" or type(v) == "string" or type(v) == "boolean" then
          payload[k] = v
          if k == "impedance_ohm" then BBC_AMP_IMPEDANCE_EXPLICIT = true end
        end
      end
    end
  end
  return payload
end

-- Snapshot of the last push, per whitelisted field, for instant change pushes
local lastPushed = {}

local function sendToGE()
  if not obj then return end
  local payload = buildPayload()
  finalizeConfig(payload)
  local ok, encoded = pcall(json.encode, payload)
  if not ok or not encoded then
    log('W', "bbcAmpConfig", "config encode failed: " .. tostring(encoded))
    return
  end
  -- remember what we pushed so update() can detect changes and push instantly
  for _, k in ipairs(FIELD_WHITELIST) do
    lastPushed[k] = payload[k]
  end
  -- single-quoted Lua literal: escape backslash and single quote
  local escaped = encoded:gsub("\\", "\\\\"):gsub("'", "\\'")
  obj:queueGameEngineLua(
    "if ampSimGE and ampSimGE.setConfigJson then ampSimGE.setConfigJson('" .. escaped .. "') end")
end

local function init(jbeamData)
  instanceName = jbeamData.name or jbeamData.fileName or "?"
  configType = jbeamData.configType or "amp"

  local applied = 0
  for k, v in pairs(jbeamData) do
    if not SKIP_KEYS[k] and (type(v) == "number" or type(v) == "string" or type(v) == "boolean") then
      bbcAmpSimValues[k] = v
      if k == "impedance_ohm" then BBC_AMP_IMPEDANCE_EXPLICIT = true end
      applied = applied + 1
    end
  end

  -- Re-push periodically (every 2s) so a startup race or GE reload can never
  -- leave the app with stale defaults. One tiny queueGameEngineLua per 2s.
  sendTimer = 2.0 -- send on the first update tick

  log('I', "bbcAmpConfig", string.format(
    "instance '%s' (configType %s) merged %d values into registry",
    instanceName, configType, applied))
end

-- Instant pushes: detect any whitelisted value change (electrics or jbeam
-- registry) against the last push and send immediately; the 2s timer stays
-- only as a heartbeat for values that arrive without us watching.
local function anyValueChanged()
  local ev = electrics and electrics.values
  for _, k in ipairs(FIELD_WHITELIST) do
    local cur = ev and ev[k]
    if cur == nil then cur = bbcAmpSimValues[k] end
    if cur ~= lastPushed[k] then return true end
  end
  return false
end

local function update(dt)
  sendTimer = sendTimer + (dt or 0)
  if anyValueChanged() then
    sendTimer = 0
    sendToGE()
    return
  end
  if sendTimer < 2.0 then return end
  sendTimer = 0
  sendToGE()
end

local function reset()
  -- parts config survives vehicle resets; re-push promptly
  sendTimer = 2.0
end

M.init = init
M.update = update
M.reset = reset

-- merged vehicle-side config, readable by other controllers/extensions
M.getValues = function() return bbcAmpSimValues end
M.getConfigType = function() return configType end
M.getInstanceName = function() return instanceName end

return M