-- bbcAmpTuner.lua
-- Vehicle controller that lets physical triggers / buttons RAISE or LOWER
-- amp/sub/box values at runtime. Mirrors the seatsController pattern:
--
--   jbeam events write momentary inputs:   electrics.values.lpf_hz_up = 1
--   this controller integrates them:       electrics.values.lpf_hz += rate*dt
--   the adjusted value flows to the app via bbcAmpConfig's electrics
--   override (app slider > electrics > jbeam > default).
--
-- BASE VALUE: seeded on the first tick from electrics.values.<field> if a
-- controller/lua already set it, otherwise from the jbeam registry
-- (bbcAmpSimValues - the merged bbcAmpConfig part data).
--
-- jbeam:
--   ["bbcAmpTuner", {"tunables": {
--       "lpf_hz": {},                    -- hold mode, built-in min/max/rate
--       "bass_boost_db": {min = 0, max = 18, rate = 3},
--       "fuse_blown": {mode = "toggle"}, -- rising-edge press flips 0/1
--   }}]
--
-- Inputs (written by triggers/events/input actions):
--   <field>_up / <field>_down   hold-to-move (momentary, rate*dt per tick)
--   <field>_press               rising edge toggles (mode = "toggle" fields)
--
-- One tuner instance per vehicle; fields are shared through the global
-- BBC_AMP_TUNER_STATE table.

local M = {}
M.type = "auxiliary"

-- Built-in ranges/rates for known fields (jbeam tunables can override all).
-- "def" = seed value when neither electrics nor the jbeam registry provides one.
local DEFAULTS = {
  gain_db         = { min = 0,   max = 32,  rate = 4,   def = 24 },
  preout_v        = { min = 0.5, max = 8,   rate = 0.25, def = 4 },
  bass_boost_db   = { min = 0,   max = 18,  rate = 3,   def = 12 },
  bass_boost_freq_hz = { min = 25, max = 80, rate = 2,  def = 45 },
  subsonic_hz     = { min = 10,  max = 60,  rate = 5,   def = 24 },
  lpf_hz          = { min = 30,  max = 250, rate = 50,  def = 80 },
  phase_deg       = { min = 0,   max = 180, rate = 45, mode = "toggle", def = 0 },
  remote_level    = { min = 0,   max = 1,   rate = 0.25, def = 0.5 },
  cabin_gain_db   = { min = 0,   max = 12,  rate = 2,   def = 6 },
  enclosure_l     = { min = 10,  max = 200, rate = 10,  def = 85 },
  port_tune_hz    = { min = 22,  max = 50,  rate = 1,   def = 34 },
  leak_pct        = { min = 0,   max = 100, rate = 10,  def = 0 },
  polyfill_pct    = { min = 0,   max = 100, rate = 10,  def = 0 },
  bracing_pct     = { min = 0,   max = 100, rate = 10,  def = 20 },
  sub_count       = { min = 1,   max = 8,   rate = 1, mode = "toggle", def = 1 },
  fuse_a          = { min = 20,  max = 300, rate = 10,  def = 150 },
  rms_power_w     = { min = 100, max = 8000, rate = 200, def = 1500 },
  impedance_ohm   = { min = 0.5, max = 8,   rate = 0.5, def = 1 },
  fuse_blown      = { min = 0,   max = 1,   mode = "toggle", def = 0 },
  thermal_protect = { min = 0,   max = 1,   mode = "toggle", def = 0 },
  bridged         = { min = 0,   max = 1,   mode = "toggle", def = 1 },
}

-- Shared per-vehicle state across instances (global registry)
BBC_AMP_TUNER_STATE = BBC_AMP_TUNER_STATE or {}

local tunables = {}

local function ensureTunable(field, spec)
  if BBC_AMP_TUNER_STATE[field] then
    return BBC_AMP_TUNER_STATE[field]
  end
  local d = DEFAULTS[field] or { min = 0, max = 100, rate = 10 }
  local t = {
    min  = tonumber(spec.min or d.min) or 0,
    max  = tonumber(spec.max or d.max) or 100,
    rate = tonumber(spec.rate or d.rate) or 10,
    mode = spec.mode or d.mode or "hold",
    def  = tonumber(spec.def or d.def),
    value = nil, -- seeded on first tick
    wasUp = false, wasDown = false, wasPress = false,
    lastLogT = 0, lastLogVal = nil,
  }
  if t.min > t.max then t.min, t.max = t.max, t.min end
  BBC_AMP_TUNER_STATE[field] = t
  return t
end

local function init(jbeamData)
  local count = 0
  for field, spec in pairs(jbeamData.tunables or {}) do
    if type(spec) ~= "table" then spec = {} end
    tunables[field] = ensureTunable(field, spec)
    count = count + 1
  end
  log('I', "bbcAmpTuner", "initialized with " .. count .. " tunable fields")
end

local function updateGFX(dt)
  if not electrics or not electrics.values then return end

  for field, t in pairs(tunables) do
    -- Seed once: live electrics value wins, else the jbeam base value
    if t.value == nil then
      local ev = electrics.values[field]
      local base = tonumber(ev)
      if base == nil and type(ev) == "boolean" then
        base = ev and 1 or 0
      end
      if base == nil and bbcAmpSimValues ~= nil then
        base = tonumber(bbcAmpSimValues[field])
      end
      t.value = math.max(t.min, math.min(t.max, base or t.def or t.min))
    end

    local up    = electrics.values[field .. "_up"] or 0
    local down  = electrics.values[field .. "_down"] or 0
    local press = electrics.values[field .. "_press"] or 0

    if t.mode == "toggle" then
      -- rising edge of the press input flips the value
      local pressed = press > 0.5
      if pressed and not t.wasPress then
        t.value = (t.value >= 1 or t.value == true) and 0 or 1
      end
      t.wasPress = pressed
    else
      if up > 0.5 and down <= 0.5 then
        t.value = math.min(t.max, t.value + t.rate * dt)
      elseif down > 0.5 and up <= 0.5 then
        t.value = math.max(t.min, t.value - t.rate * dt)
      end
    end

    -- Always write the current value so bbcAmpConfig's electrics override
    -- picks it up (booleans as 0/1 for electrics compatibility)
    electrics.values[field] = (t.mode == "toggle") and (t.value >= 1 and 1 or 0) or t.value

    -- visibility: log when a value moves (throttled to 1 line/s per field)
    if t.value ~= t.lastLogVal and os.clock() >= (t.lastLogT or 0) then
      t.lastLogVal = t.value
      t.lastLogT = os.clock() + 1
      log('I', "bbcAmpTuner", field .. " = " .. tostring(t.value))
    end
  end
end

local function reset()
  -- keep current values; just clear edge-tracking
  for _, t in pairs(tunables) do
    t.wasUp, t.wasDown, t.wasPress = false, false, false
  end
end

M.init = init
M.updateGFX = updateGFX
M.reset = reset

-- Read/adjust programmatically (screen apps, other controllers)
M.getTunable = function(field) local t = tunables[field] or BBC_AMP_TUNER_STATE[field]; return t and t.value or nil end
M.setTunable = function(field, v)
  local t = tunables[field] or BBC_AMP_TUNER_STATE[field]
  if not t or v == nil then return false end
  local n = tonumber(v)
  if n == nil and v == false then n = 0 elseif n == nil and v == true then n = 1 end
  if n == nil then return false end
  t.value = math.max(t.min, math.min(t.max, n))
  electrics.values[field] = (t.mode == "toggle") and (t.value >= 1 and 1 or 0) or t.value
  return true
end
M.getTunables = function() return tunables end

return M