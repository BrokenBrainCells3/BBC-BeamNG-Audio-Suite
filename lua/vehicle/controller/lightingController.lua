-- This Source Code Form is subject to the terms of the bCDDL, v. 1.1.
-- If a copy of the bCDDL was not distributed with this
-- file, You can obtain one at http://beamng.com/bCDDL-1.1.txt

-- LIGHTING CONTROLLER - FIXED VERSION
-- Fixes: Race conditions, unnecessary frame loops, state desync on reload
-- Added: Per-vehicle lighting config persistence (like screenService/speakerCalibration)

local M = {}
M.type = "auxiliary"

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "lightingController", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

-- Escape string for safe embedding in Lua string literal
local function escapeLuaString(s)
  return s:gsub("\\", "\\\\"):gsub('"', '\\"'):gsub("\n", "\\n"):gsub("\r", "\\r")
end

-- ============================================================
-- Internal state
-- ============================================================

-- Map of funcName -> list of { pid, flareScale }
local lightPids = {}

-- Current desired state per func name: { enabled, brightness, r, g, b, dirty }
-- enabled: whether the toggle is ON (independent of brightness)
local lightState = {}

-- All func names this controller manages
local LIGHT_FUNCS = {
  "footwell_l",
  "footwell_r",
  "footwell_rear_l",
  "footwell_rear_r",
  "dashboard_lights",
  "door_l",
  "door_r",
  "cupholder",
  "storage",
}

-- Vehicle name for persistence
local currentVehicleName = nil

-- ============================================================
-- PHYSICAL UNITS
-- ============================================================
-- Prop lights (SPOTLIGHT / POINTLIGHT) have no "nits" setting. The engine's
-- raw brightness is derived in common/jbeam/sections/meshs.lua as:
--   SPOTLIGHT : brightness = candela / 5000
--   POINTLIGHT: brightness = (lumen / 5000) / (4 * pi)
-- Nits (cd/m^2) describe the luminance of a SURFACE, so to use them you also
-- need the emitting area:  candela = nits * area_m2
-- (a point light radiates equally in all directions, so lumen = candela * 4pi
--  and its brightness works out to candela / 5000 as well)
local CD_PER_BRIGHTNESS = 5000

local function cdToBrightness(cd)   return cd / CD_PER_BRIGHTNESS end
local function lmToBrightness(lm)   return lm / CD_PER_BRIGHTNESS / (4 * math.pi) end
local function nitsToCd(nits, areaM2) return nits * areaM2 end

-- Intensity of each func at brightness level 1.0 (the "100%" of the UI slider).
-- Give EITHER maxCd, OR maxLm (point lights), OR nits + areaM2 (emitting area).
-- Defaults of maxCd = 5000 reproduce the old behaviour exactly (raw brightness 1.0).
-- TUNE THESE to taste.
local DEFAULT_SPEC = { maxCd = 5000 }
local LIGHT_SPEC = {
  -- footwell_l       = { nits = 300, areaM2 = 0.01 },
  -- cupholder        = { maxLm = 40 },
  -- Dashboard used to be dimmed by scaling RGB by 0.03 (which crushes colour
  -- resolution to ~8 steps). Dim it with intensity instead: 0.03 * 5000 = 150 cd.
  dashboard_lights = { maxCd = 5000 },
}

-- UI level (0..1+) -> raw brightness handed to obj:setPropLight
local function levelToRawBrightness(funcName, level)
  local spec = LIGHT_SPEC[funcName] or DEFAULT_SPEC
  local cd
  if spec.maxCd then
    cd = spec.maxCd
  elseif spec.maxLm then
    cd = spec.maxLm / (4 * math.pi) -- lumens -> candela (isotropic)
  elseif spec.nits and spec.areaM2 then
    cd = nitsToCd(spec.nits, spec.areaM2)
  else
    cd = DEFAULT_SPEC.maxCd
  end
  return cdToBrightness(cd) * level
end

-- ============================================================
-- LIGHTING MODES STATE
-- ============================================================
local lightingMode = "static" -- static, breathing, rainbow, music
local rainbowOffset = 0 -- For rainbow mode animation
local breathingPhase = 0 -- For breathing mode animation

-- Time accumulator for fallback pulse effect
local timeAccumulator = 0

-- ============================================================
-- BUG FIX #6: Dirty count tracking (avoids per-frame iteration)
-- ============================================================
local dirtyCount = 0

-- Last brightness / main color received from the screen app. Used as fallback
-- when an update (e.g. a song-change dashboard recolor) doesn't include them.
local lastBrightness = 0.75
local lastColor = { r = 255, g = 255, b = 255 }

-- Door state tracking for auto footwell lighting
local prevDoorState = { FL = 0, FR = 0, RL = 0, RR = 0 }

-- Track which footwells were auto-enabled (so we don't turn off manually enabled ones)
local autoEnabledFootwell = { footwell_l = false, footwell_r = false, footwell_rear_l = false, footwell_rear_r = false }

-- Ignition state tracking for screen power management
local prevIgnitionLevel = 1
local bootAnimationPlayed = false

-- ============================================================
-- Send lighting prop info to GE Lua (for lightingPropsEditor)
-- Must be defined before buildPidMap which calls it
-- ============================================================
local function sendLightingPropsToGE()
  dlog('I', 'lightingController.sendLightingPropsToGE', 'Sending lighting props to GE')
  local propsData = v.data.props
  if not propsData then return end
  
  local lightProps = {}
  for i = 0, tableSizeC(propsData) - 1 do
    local prop = propsData[i]
    if prop.pid and (prop.mesh == "SPOTLIGHT" or prop.mesh == "POINTLIGHT") then
      table.insert(lightProps, {
        func = prop.func,
        mesh = prop.mesh,
        pid = prop.pid,
        pos = prop.baseTranslation or {x=0,y=0,z=0},
        rot = prop.baseRotation or {x=0,y=0,z=0},
        color = prop.lightColor or {r=255,g=255,b=255},
        brightness = prop.lightBrightness or 1.0,
        range = prop.lightRange or 1.0,
        innerAngle = prop.lightInnerAngle or 45,
        outerAngle = prop.lightOuterAngle or 60,
        castShadows = prop.lightCastShadows or false,
        electric = prop.electric,
        idRef = prop.idRef,
        idX = prop.idX,
        idY = prop.idY,
        partPath = prop.partPath
      })
    end
  end
  
  -- Send directly to GE Lua screenService
  local encodedData = lpack.encode({props = lightProps})
  obj:queueGameEngineLua(string.format(
    'if screenService and screenService.onLightingPropsData then screenService.onLightingPropsData(lpack.decode("%s")) end',
    escapeLuaString(encodedData)
  ))
end

-- ============================================================
-- Internal helpers
-- ============================================================

local function buildPidMap()
  dlog('I', 'lightingController.buildPidMap', 'buildPidMap called')
  lightPids = {}
  lightState = {}
  dirtyCount = 0  -- Reset dirty count
  lastBrightness = 0.75
  lastColor = { r = 255, g = 255, b = 255 }

  local propsData = v.data.props
  if not propsData then
    dlog('W', 'interiorAmbientLights', 'No props data found on vehicle')
    return
  end

  -- Pre-populate so every func has an entry even if no props found
  -- Start with all lights OFF (enabled = false, brightness = 0)
  for _, funcName in ipairs(LIGHT_FUNCS) do
    lightPids[funcName] = {}
    lightState[funcName] = { enabled = false, brightness = 0.0, r = 255, g = 255, b = 255, dirty = false }
    -- Initialize electric value to 0 (off)
    electrics.values[funcName] = 0
  end

  for i = 0, tableSizeC(propsData) - 1 do
    local prop = propsData[i]
    if prop.pid and (prop.mesh == "SPOTLIGHT" or prop.mesh == "POINTLIGHT") then
      local entry = lightPids[prop.func]
      if entry then
        table.insert(entry, { pid = prop.pid, flareScale = 0.0 })
        dlog('I', 'interiorAmbientLights',
          string.format("Found prop: func='%s'  pid=%d  mesh=%s  flareScale=%.2f",
            prop.func, prop.pid, prop.mesh, 0.0))
      end
    end
  end

  -- Summary: report any func names that got zero props
  for _, funcName in ipairs(LIGHT_FUNCS) do
    local count = #lightPids[funcName]
    if count == 0 then
      dlog('W', 'interiorAmbientLights', 'No props found for func: ' .. funcName)
    else
      dlog('I', 'interiorAmbientLights',
        string.format("func='%s' -> %d prop(s) registered", funcName, count))
    end
  end
  
  -- Send prop data to GE Lua for lightingPropsEditor
  dlog('I', 'lightingController.buildPidMap', 'About to call sendLightingPropsToGE')
  if sendLightingPropsToGE then
    sendLightingPropsToGE()
  else
    dlog('E', 'lightingController.buildPidMap', 'sendLightingPropsToGE is nil!')
  end
end

local function applyLight(funcName)
  local pids  = lightPids[funcName]
  local state = lightState[funcName]
  if not pids or not state then return end

  -- ONLY set electric based on enabled (toggle state), nothing else
  electrics.values[funcName] = state.enabled and 1 or 0

  -- Only apply prop light if enabled
  if state.enabled then
    local c = color(state.r, state.g, state.b, 255)
    local raw = levelToRawBrightness(funcName, state.brightness)
    for _, p in ipairs(pids) do
      obj:setPropLight(p.pid, raw, 0.0, c)
    end
  else
    local off = color(0, 0, 0, 0)
    for _, p in ipairs(pids) do
      obj:setPropLight(p.pid, 0, 0.0, off)
    end
  end

  state.dirty = false
  -- BUG FIX #6: Decrement dirty count
  if dirtyCount > 0 then
    dirtyCount = dirtyCount - 1
  end

  dlog('D', 'lightingController.applyLight',
    string.format("func='%s' enabled=%s electric=%d", funcName, tostring(state.enabled), electrics.values[funcName]))
end

-- ============================================================
-- Persistence (load/save per-vehicle lighting config)
-- ============================================================

local function getVehicleName()
  if currentVehicleName then return currentVehicleName end
  -- In vehicle Lua, v.data.model contains the JBeam filename
  local vehName = v.data and v.data.model
  if vehName and vehName ~= "" then
    currentVehicleName = vehName
    return vehName
  end
  -- Fallback: try obj:getJBeamFilename() if available
  if obj and obj.getJBeamFilename then
    local ok, name = pcall(obj.getJBeamFilename, obj)
    if ok and name and name ~= "" then
      currentVehicleName = name
      return name
    end
  end
  return nil
end

-- Load saved lighting config from GE (async via queueGameEngineLua)
local function loadLightingConfig()
  local vehName = getVehicleName()
  if not vehName then
    dlog('W', 'lightingController', 'No vehicle name available for loading config')
    return false
  end

  -- Get vehicle ID for GE Lua callback
  local vehId = obj:getID()
  if not vehId then
    dlog('W', 'lightingController', 'No vehicle ID available for loading config')
    return false
  end

  -- Queue GE to load config and call back into vehicle
  local jsonStr = escapeLuaString(lpack.encode({vehName = vehName}))
  obj:queueGameEngineLua(string.format(
    'if extensions.lightingCalibration then ' ..
    '  local data, source = extensions.lightingCalibration.load("%s") ' ..
    '  if data then ' ..
    '    local veh = be:getObjectByID(%d) ' ..
    '    if veh then veh:queueLuaCommand("lightingControllerLoadConfigCallback(\\"' .. jsonStr .. '\\", \\"" .. lpack.encode(data) .. "\\")") end ' ..
    '  end ' ..
    'end', vehName, vehId
  ))
  return true
end

-- Apply loaded config to lightState
local function applyLightingConfig(cfg)
  if not cfg then return end

  -- Restore zones (which lights are on/off)
  if cfg.zones then
    -- Footwell
    if cfg.zones.footwell then
      setLight('footwell_l', cfg.brightness or 0.75, 
        (cfg.customColor and cfg.customColor.r) or 255,
        (cfg.customColor and cfg.customColor.g) or 255,
        (cfg.customColor and cfg.customColor.b) or 255)
      setLight('footwell_r', cfg.brightness or 0.75,
        (cfg.customColor and cfg.customColor.r) or 255,
        (cfg.customColor and cfg.customColor.g) or 255,
        (cfg.customColor and cfg.customColor.b) or 255)
      setLight('footwell_rear_l', cfg.brightness or 0.75,
        (cfg.customColor and cfg.customColor.r) or 255,
        (cfg.customColor and cfg.customColor.g) or 255,
        (cfg.customColor and cfg.customColor.b) or 255)
      setLight('footwell_rear_r', cfg.brightness or 0.75,
        (cfg.customColor and cfg.customColor.r) or 255,
        (cfg.customColor and cfg.customColor.g) or 255,
        (cfg.customColor and cfg.customColor.b) or 255)
    else
      turnOff('footwell_l')
      turnOff('footwell_r')
      turnOff('footwell_rear_l')
      turnOff('footwell_rear_r')
    end

    -- Dashboard
    if cfg.zones.dashboard then
      setLight('dashboard_lights', 1.0,
        (cfg.dashboardColor and cfg.dashboardColor.r) or 255,
        (cfg.dashboardColor and cfg.dashboardColor.g) or 255,
        (cfg.dashboardColor and cfg.dashboardColor.b) or 255)
    else
      turnOff('dashboard_lights')
    end

    -- Doors
    if cfg.zones.doors then
      setLight('door_l', cfg.brightness or 0.75,
        (cfg.customColor and cfg.customColor.r) or 255,
        (cfg.customColor and cfg.customColor.g) or 255,
        (cfg.customColor and cfg.customColor.b) or 255)
      setLight('door_r', cfg.brightness or 0.75,
        (cfg.customColor and cfg.customColor.r) or 255,
        (cfg.customColor and cfg.customColor.g) or 255,
        (cfg.customColor and cfg.customColor.b) or 255)
    else
      turnOff('door_l')
      turnOff('door_r')
    end

    -- Cupholder
    if cfg.zones.cupholder then
      setLight('cupholder', cfg.brightness or 0.75,
        (cfg.customColor and cfg.customColor.r) or 255,
        (cfg.customColor and cfg.customColor.g) or 255,
        (cfg.customColor and cfg.customColor.b) or 255)
    else
      turnOff('cupholder')
    end

    -- Storage
    if cfg.zones.storage then
      setLight('storage', cfg.brightness or 0.75,
        (cfg.customColor and cfg.customColor.r) or 255,
        (cfg.customColor and cfg.customColor.g) or 255,
        (cfg.customColor and cfg.customColor.b) or 255)
    else
      turnOff('storage')
    end
  end

  -- Restore mode
  if cfg.mode then
    lightingMode = cfg.mode
    dlog('I', 'lightingController', 'Restored lighting mode: ' .. lightingMode)
  end

  -- Force apply all dirty lights immediately
  for _, funcName in ipairs(LIGHT_FUNCS) do
    local state = lightState[funcName]
    if state and state.dirty then
      applyLight(funcName)
    end
  end
end

-- Callback from GE with loaded config data
local function loadLightingConfigCallback(_, data)
  if not data then return end
  applyLightingConfig(data)
  dlog('I', 'lightingController', 'Loaded lighting config from GE callback')
  -- Config arrives async after reset(); tell the screen what we actually restored
  if M.getLightingState then M.getLightingState() end
end

-- Make callback globally accessible for queueLuaCommand
rawset(_G, "lightingControllerLoadConfigCallback", loadLightingConfigCallback)

-- Save current lighting state to config (via GE)
local function saveLightingConfig()
  local vehName = getVehicleName()
  if not vehName then
    dlog('W', 'lightingController', 'No vehicle name available for saving config')
    return false
  end

  -- Build current state from lightState
  local cfg = {
    zones = {
      footwell = (lightState['footwell_l'] and lightState['footwell_l'].enabled) or
                 (lightState['footwell_r'] and lightState['footwell_r'].enabled),
      dashboard = lightState['dashboard_lights'] and lightState['dashboard_lights'].enabled,
      doors = (lightState['door_l'] and lightState['door_l'].enabled) or
              (lightState['door_r'] and lightState['door_r'].enabled),
      cupholder = lightState['cupholder'] and lightState['cupholder'].enabled,
      storage = lightState['storage'] and lightState['storage'].enabled
    },
    brightness = lightState['footwell_l'] and lightState['footwell_l'].brightness or 0.75,
    customColor = lightState['footwell_l'] and {
      r = lightState['footwell_l'].r,
      g = lightState['footwell_l'].g,
      b = lightState['footwell_l'].b
    } or {r = 255, g = 255, b = 255},
    dashboardColor = lightState['dashboard_lights'] and {
      r = lightState['dashboard_lights'].r,
      g = lightState['dashboard_lights'].g,
      b = lightState['dashboard_lights'].b
    } or {r = 255, g = 255, b = 255},
    mode = lightingMode
  }

  -- Queue GE to save config via lightingCalibration extension
  local cfgStr = escapeLuaString(lpack.encode(cfg))
  local vehStr = escapeLuaString(vehName)
  obj:queueGameEngineLua(string.format(
    'if extensions.lightingCalibration then ' ..
    '  extensions.lightingCalibration.save("%s", lpack.decode("%s")) ' ..
    'end', vehName, cfgStr
  ))
  
  dlog('I', 'lightingController', 'Queued lighting config save for ' .. vehName)
  return true
end

-- ============================================================
-- Public API
-- ============================================================

-- Set a single light group by func name
-- brightness: 0.0 (off) to 1.0+ (full/over-bright)
-- r, g, b:    0-255 integers
local function setLight(funcName, brightness, r, g, b)
  local state = lightState[funcName]
  if not state then
    dlog('W', 'interiorAmbientLights', 'Unknown func name: ' .. tostring(funcName))
    return
  end
  
  -- Only mark dirty if state actually changed (reduces unnecessary updates)
  local changed = not state.enabled or state.brightness ~= brightness or 
                  state.r ~= r or state.g ~= g or state.b ~= b
  
  state.enabled    = true   -- Toggle is ON
  state.brightness = brightness
  state.r          = r
  state.g          = g
  state.b          = b
  
  -- BUG FIX: Only increment dirty if not already dirty
  if not state.dirty and changed then
    state.dirty = true
    dirtyCount = dirtyCount + 1
  end
  
  dlog('D', 'lightingController.setLight', string.format("func='%s' enabled=true brightness=%.2f dirty=%s", funcName, brightness, tostring(changed)))
end

-- Set all managed lights to the same brightness and color at once
local function setAll(brightness, r, g, b)
  for _, funcName in ipairs(LIGHT_FUNCS) do
    setLight(funcName, brightness, r, g, b)
  end
end

-- Set just the brightness of a light, keeping its current color
local function setBrightness(funcName, brightness)
  local state = lightState[funcName]
  if not state then
    dlog('W', 'interiorAmbientLights', 'Unknown func name: ' .. tostring(funcName))
    return
  end

  local changed = state.brightness ~= brightness or not state.enabled
  if changed then
    state.brightness = brightness
    state.enabled    = true
    if not state.dirty then
      state.dirty = true
      dirtyCount = dirtyCount + 1
    end
  end
end

-- Set just the RGB color of a light, keeping its current brightness
local function setColor(funcName, r, g, b)
  local state = lightState[funcName]
  if not state then
    dlog('W', 'interiorAmbientLights', 'Unknown func name: ' .. tostring(funcName))
    return
  end
  
  -- Only mark dirty if color changed
  if state.r ~= r or state.g ~= g or state.b ~= b then
    state.r     = r
    state.g     = g
    state.b     = b
    if not state.dirty then
      state.dirty = true
      dirtyCount = dirtyCount + 1
    end
  end
end

-- Turn a light off (sets enabled = false, electric = 0)
local function turnOff(funcName)
  local state = lightState[funcName]
  if not state then
    dlog('W', 'interiorAmbientLights', 'Unknown func name: ' .. tostring(funcName))
    return
  end
  
  -- Only mark dirty if was enabled
  if state.enabled then
    state.enabled = false  -- Toggle is OFF
    if not state.dirty then
      state.dirty = true
      dirtyCount = dirtyCount + 1
    end
    dlog('D', 'lightingController.turnOff', string.format("func='%s' enabled=false dirty=true", funcName))
  end
end

-- Turn a light back on to full brightness without changing its color
local function turnOn(funcName)
  local state = lightState[funcName]
  if not state then
    dlog('W', 'interiorAmbientLights', 'Unknown func name: ' .. tostring(funcName))
    return
  end
  
  -- Only mark dirty if was off or brightness != 1.0
  if not state.enabled or state.brightness ~= 1.0 then
    state.enabled    = true  -- Toggle is ON
    state.brightness = 1.0
    if not state.dirty then
      state.dirty = true
      dirtyCount = dirtyCount + 1
    end
  end
end

-- Turn all lights off
local function allOff()
  for _, funcName in ipairs(LIGHT_FUNCS) do
    turnOff(funcName)
  end
end

-- Turn all lights on (full brightness, current colors)
local function allOn()
  for _, funcName in ipairs(LIGHT_FUNCS) do
    turnOn(funcName)
  end
end

-- Check door states and auto-turn on/off footwell lights when doors open/close
local function checkDoorFootwellAuto()
  -- Door electrics: 0 = closed/attached, 1 = open/detached
  local doorFL = electrics.values.door_FL_coupler_notAttached or 0
  local doorFR = electrics.values.door_FR_coupler_notAttached or 0
  local doorRL = electrics.values.door_RL_coupler_notAttached or 0
  local doorRR = electrics.values.door_RR_coupler_notAttached or 0
  
  -- Door-to-footwell mapping
  local doors = {
    { door = doorFL, prev = prevDoorState.FL, footwell = 'footwell_l', name = 'FL' },
    { door = doorFR, prev = prevDoorState.FR, footwell = 'footwell_r', name = 'FR' },
    { door = doorRL, prev = prevDoorState.RL, footwell = 'footwell_rear_l', name = 'RL' },
    { door = doorRR, prev = prevDoorState.RR, footwell = 'footwell_rear_r', name = 'RR' },
  }
  
  for _, d in ipairs(doors) do
    local state = lightState[d.footwell]
    
    -- Door opened (0 -> 1): turn on footwell if it was off
    if d.door == 1 and d.prev == 0 then
      if state and not state.enabled then
        setLight(d.footwell, lastBrightness, lastColor.r, lastColor.g, lastColor.b)
        autoEnabledFootwell[d.footwell] = true
        dlog('I', 'lightingController', 'Auto-on: ' .. d.name .. ' door opened -> ' .. d.footwell .. ' ON')
      end
    -- Door closed (1 -> 0): turn off footwell if it was on AND was auto-enabled
    elseif d.door == 0 and d.prev == 1 then
      if state and state.enabled and autoEnabledFootwell[d.footwell] then
        turnOff(d.footwell)
        autoEnabledFootwell[d.footwell] = false
        dlog('I', 'lightingController', 'Auto-off: ' .. d.name .. ' door closed -> ' .. d.footwell .. ' OFF')
      end
    end
    
    -- Update previous state
    if d.name == 'FL' then prevDoorState.FL = d.door
    elseif d.name == 'FR' then prevDoorState.FR = d.door
    elseif d.name == 'RL' then prevDoorState.RL = d.door
    elseif d.name == 'RR' then prevDoorState.RR = d.door
    end
  end
end

-- ============================================================
-- GFX update - Includes mode animations (rainbow, breathing)
-- ============================================================

local modeUpdateTimer = 0
local modeUpdateInterval = 1 / 30 -- 30 FPS for mode animations

local function updateGFX(dt)
  -- Check door states for auto footwell lighting
  checkDoorFootwellAuto()
  
  -- Update time accumulator
  timeAccumulator = timeAccumulator + dt

  -- Update mode animations (rainbow, breathing) - BUT NOT FOR DASHBOARD
  modeUpdateTimer = modeUpdateTimer + dt
  if modeUpdateTimer >= modeUpdateInterval then
    modeUpdateTimer = modeUpdateTimer - modeUpdateInterval

    -- Rainbow mode: cycle through colors (NOT for dashboard)
    if lightingMode == "rainbow" then
      rainbowOffset = rainbowOffset + 0.05
      if rainbowOffset > 1 then
        rainbowOffset = rainbowOffset - 1
      end

      -- Update all enabled lights EXCEPT dashboard with rainbow colors
      for i, funcName in ipairs(LIGHT_FUNCS) do
        -- Skip dashboard lights - they always use screen color
        if funcName == 'dashboard_lights' then
          goto continue
        end

        local state = lightState[funcName]
        if state and state.enabled then
          -- Calculate rainbow color based on position and time
          -- Each light gets a slightly offset hue for a cascading effect
          local hueOffset = (i * 0.15) % 1
          local hue = (rainbowOffset + hueOffset) % 1
          local r = math.floor(255 *
                                       (0.5 + 0.5 *
                                           math.sin(hue * 2 * math.pi)))
          local g = math.floor(255 *
                                       (0.5 + 0.5 *
                                           math.sin(
                                               (hue + 0.33) * 2 * math.pi)))
          local b = math.floor(255 *
                                       (0.5 + 0.5 *
                                           math.sin(
                                               (hue + 0.67) * 2 * math.pi)))

          if state.r ~= r or state.g ~= g or state.b ~= b then
            state.r = r
            state.g = g
            state.b = b
            state.dirty = true
            dirtyCount = dirtyCount + 1
          end
        end
        ::continue::
      end
    end

    -- Breathing mode: pulse brightness (NOT for dashboard)
    if lightingMode == "breathing" then
      breathingPhase = breathingPhase + 0.03
      local breathValue = 0.5 + 0.5 *
                                  math.sin(breathingPhase * 2 * math.pi)

      -- Update all enabled lights EXCEPT dashboard with breathing brightness
      for _, funcName in ipairs(LIGHT_FUNCS) do
        -- Skip dashboard lights - they always use screen color
        if funcName == 'dashboard_lights' then
          goto continue
        end

        local state = lightState[funcName]
        if state and state.enabled then
          local targetBrightness = 0.3 + 0.7 * breathValue
          if math.abs(state.brightness - targetBrightness) > 0.01 then
            state.brightness = targetBrightness
            state.dirty = true
            dirtyCount = dirtyCount + 1
          end
        end
        ::continue::
      end
    end
  end -- End of 30 FPS timer block

  -- Apply dirty state changes
  if dirtyCount > 0 then
    for _, funcName in ipairs(LIGHT_FUNCS) do
      local state = lightState[funcName]
      if state and state.dirty then applyLight(funcName) end
    end
  end

  -- Ignition state tracking for screen power management
  -- ignitionLevel: 0=off, 1=accessory, 2=ignition on, 3=starter
  local ignitionLevel = electrics.values.ignitionLevel or 0
  if ignitionLevel ~= prevIgnitionLevel then
    dlog('I', 'lightingController', 'Ignition level changed: ' .. prevIgnitionLevel .. ' -> ' .. ignitionLevel)
    
    -- Car turned off - send black screen command
    if ignitionLevel == 0 then
      dlog('I', 'lightingController', 'Queueing onIgnitionOff via screenInput.queueIgnitionEvent')
      if screenInput and screenInput.queueIgnitionEvent then
        screenInput.queueIgnitionEvent('onIgnitionOff', {})
      end
      -- Reset boot animation flag when car turns off
      bootAnimationPlayed = false
    else
      -- Car turned on (accessory, ignition, or starter)
      -- Only play boot animation if transitioning from 0 (off) to non-zero
      if prevIgnitionLevel == 0 and not bootAnimationPlayed then
        dlog('I', 'lightingController', 'Queueing onIgnitionOn via screenInput.queueIgnitionEvent with level ' .. ignitionLevel)
        if screenInput and screenInput.queueIgnitionEvent then
          screenInput.queueIgnitionEvent('onIgnitionOn', { ignitionLevel = ignitionLevel })
        end
        bootAnimationPlayed = true
      end
    end
    prevIgnitionLevel = ignitionLevel
  end
end

-- ============================================================
-- High-level update function (called from JavaScript via triggerExample)
-- MUST be defined BEFORE init() so it can be registered as a callback
-- ============================================================

local function updateLighting(args)
  if not args then
    dlog('W', 'lightingController.updateLighting', 'No args provided')
    return
  end

  -- Brightness: use what was sent, otherwise the last value we were given
  local brightness = args.brightness
  if brightness then
    lastBrightness = brightness
  else
    brightness = lastBrightness
  end

  -- Main color: use what was sent, otherwise keep the last one
  if args.customColor then
    lastColor = {
      r = args.customColor.r or 255,
      g = args.customColor.g or 255,
      b = args.customColor.b or 255,
    }
  elseif args.color then
    local hex = args.color:gsub('#', '')
    lastColor = {
      r = tonumber(hex:sub(1, 2), 16) or 255,
      g = tonumber(hex:sub(3, 4), 16) or 255,
      b = tonumber(hex:sub(5, 6), 16) or 255,
    }
  end
  local r, g, b = lastColor.r, lastColor.g, lastColor.b

  -- Dashboard light color (separate from main lights) - uses dynamic color from media
  local dashR, dashG, dashB = 255, 255, 255
  if args.dashboardColor then
    dashR = args.dashboardColor.r or 255
    dashG = args.dashboardColor.g or 255
    dashB = args.dashboardColor.b or 255
    dlog('I', 'lightingController.updateLighting', 'Received dashboardColor: RGB(' .. dashR .. ',' .. dashG .. ',' .. dashB .. ')')
  elseif args.customColor then
    -- Fallback to customColor if dashboardColor not provided
    dashR = args.customColor.r or 255
    dashG = args.customColor.g or 255
    dashB = args.customColor.b or 255
    dlog('W', 'lightingController.updateLighting', 'No dashboardColor received, falling back to customColor')
  else
    dlog('W', 'lightingController.updateLighting', 'No dashboardColor or customColor received, using white')
  end

  -- Zones: per-zone merge. Explicit true/false from the caller wins; any zone
  -- the caller didn't specify keeps its current state.
  local function isOn(...)
    for _, f in ipairs({...}) do
      if lightState[f] and lightState[f].enabled then return true end
    end
    return false
  end

  local current = {
    footwell  = isOn('footwell_l', 'footwell_r', 'footwell_rear_l', 'footwell_rear_r'),
    dashboard = isOn('dashboard_lights'),
    doors     = isOn('door_l', 'door_r'),
    cupholder = isOn('cupholder'),
    storage   = isOn('storage'),
  }

  local given = args.zones or {}
  local zones = {}
  for k, v in pairs(current) do
    if given[k] ~= nil then zones[k] = given[k] else zones[k] = v end
  end

  -- If user explicitly turns on footwell via UI, clear auto-enabled flags
  -- so auto-off won't turn them off when doors close
  if given.footwell == true then
    autoEnabledFootwell.footwell_l = false
    autoEnabledFootwell.footwell_r = false
    autoEnabledFootwell.footwell_rear_l = false
    autoEnabledFootwell.footwell_rear_r = false
    dlog('D', 'lightingController.updateLighting', 'Footwell manually enabled via UI, clearing auto flags')
  end

  -- Get lighting mode (static, breathing, rainbow, music)
  if args.mode then
    lightingMode = args.mode
    dlog('I', 'lightingController.updateLighting',
        'Mode set to: ' .. lightingMode)
  end

  dlog('I', 'lightingController.updateLighting', '=== UPDATE START ===')
  dlog('I', 'lightingController.updateLighting', 'Received: footwell=' .. tostring(zones.footwell) .. ' dashboard=' .. tostring(zones.dashboard) .. ' doors=' .. tostring(zones.doors) .. ' cupholder=' .. tostring(zones.cupholder) .. ' storage=' .. tostring(zones.storage))

  -- Footwell lights - check explicitly for true
  local footwellOn = (zones.footwell == true)
  dlog('D', 'lightingController.updateLighting', 'Footwell: ' .. tostring(footwellOn))
  if footwellOn then
    setLight('footwell_l', brightness, r, g, b)
    setLight('footwell_r', brightness, r, g, b)
    setLight('footwell_rear_l', brightness, r, g, b)
    setLight('footwell_rear_r', brightness, r, g, b)
  else
    turnOff('footwell_l')
    turnOff('footwell_r')
    turnOff('footwell_rear_l')
    turnOff('footwell_rear_r')
  end

  -- Dashboard lights
  local dashboardOn = (zones.dashboard == true)
  dlog('D', 'lightingController.updateLighting', 'Dashboard: ' .. tostring(dashboardOn))
  if dashboardOn then
    -- Dimming is now done by LIGHT_SPEC.dashboard_lights.maxCd, so the colour keeps full resolution
    setLight('dashboard_lights', 1.0, dashR, dashG, dashB)
    dlog('I', 'lightingController.updateLighting',
        'Dashboard lights updated with dynamic color: RGB(' .. dashR .. ',' .. dashG .. ',' .. dashB .. ')')
  else
    turnOff('dashboard_lights')
  end

  -- Door lights
  local doorsOn = (zones.doors == true)
  dlog('D', 'lightingController.updateLighting', 'Doors: ' .. tostring(doorsOn))
  if doorsOn then
    setLight('door_l', brightness, r, g, b)
    setLight('door_r', brightness, r, g, b)
  else
    turnOff('door_l')
    turnOff('door_r')
  end

  -- Cupholder light
  local cupholderOn = (zones.cupholder == true)
  dlog('D', 'lightingController.updateLighting', 'Cupholder: ' .. tostring(cupholderOn))
  if cupholderOn then
    setLight('cupholder', brightness, r, g, b)
  else
    turnOff('cupholder')
  end

  -- Storage light
  local storageOn = (zones.storage == true)
  dlog('D', 'lightingController.updateLighting', 'Storage: ' .. tostring(storageOn))
  if storageOn then
    setLight('storage', brightness, r, g, b)
  else
    turnOff('storage')
  end

  dlog('I', 'lightingController.updateLighting', '=== UPDATE END ===')
  dlog('I', 'lightingController.updateLighting', 'Dirty count after update: ' .. tostring(dirtyCount))

  -- Save lighting config so it persists across vehicle resets/spawns
  saveLightingConfig()
end

-- Get current lighting state (reads from lightState, not electrics)
-- MUST be defined BEFORE init() so it can be registered as a callback
local function getLightingState()
  -- Build zone states from lightState (source of truth)
  local function en(name) return lightState[name] and lightState[name].enabled == true or false end
  local zones = {
    footwell  = en('footwell_l') or en('footwell_r') or en('footwell_rear_l') or en('footwell_rear_r'),
    dashboard = en('dashboard_lights'),
    doors     = en('door_l') or en('door_r'),
    cupholder = en('cupholder'),
    storage   = en('storage')
  }

  -- Get brightness from footwell_l as representative value
  local brightness = lightState['footwell_l'] and lightState['footwell_l'].brightness or 0.75

  -- Get color from footwell_l as representative value
  local customColor = nil
  if lightState['footwell_l'] then
    customColor = {
      r = lightState['footwell_l'].r,
      g = lightState['footwell_l'].g,
      b = lightState['footwell_l'].b
    }
  end

  -- Get dashboard color from dashboard_lights as representative value
  local dashboardColor = nil
  if lightState['dashboard_lights'] then
    dashboardColor = {
      r = lightState['dashboard_lights'].r,
      g = lightState['dashboard_lights'].g,
      b = lightState['dashboard_lights'].b
    }
  end

  local state = {
    zones = zones,
    brightness = brightness,
    customColor = customColor,
    dashboardColor = dashboardColor,
    mode = lightingMode
  }

  dlog('D', 'lightingController.getLightingState',
    string.format('Sending state: footwell=%s dashboard=%s doors=%s cupholder=%s storage=%s mode=%s',
      tostring(zones.footwell), tostring(zones.dashboard), tostring(zones.doors),
      tostring(zones.cupholder), tostring(zones.storage), tostring(lightingMode)))

  -- Send to JavaScript (pushToScreens is the long-standing exported path; callJS is also exported now)
  if screenInput then
    local send = screenInput.pushToScreens or screenInput.callJS
    if send then
      send('onLightingStateUpdate', state)
    else
      dlog('E', 'lightingController.getLightingState', 'screenInput has no pushToScreens/callJS')
    end
  end
end

-- ============================================================
-- Callback registration (basspeakerpos pattern)
-- ============================================================

local callbacksRegistered = false
local initRetryCount = 0
local MAX_INIT_RETRIES = 50

-- Deferred initialization: retry registering callbacks if screenInput wasn't ready
local function registerCallbacks()
  if callbacksRegistered then
    return true
  end

  if not screenInput then
    initRetryCount = initRetryCount + 1
    if initRetryCount < MAX_INIT_RETRIES then
      dlog("D", "lightingController.registerCallbacks", 
          "screenInput not available, retry " .. initRetryCount .. "/" .. MAX_INIT_RETRIES)
      -- Queue a retry after a short delay
      if obj then
        obj:queueLuaCommand("if extensions and extensions.lightingControllerRetry then extensions.lightingControllerRetry() end")
      end
    else
      dlog("E", "lightingController.registerCallbacks", 
          "Failed to register callbacks after " .. MAX_INIT_RETRIES .. " retries")
    end
    return false
  end

  if not screenInput.registerLuaCallback then
    initRetryCount = initRetryCount + 1
    if initRetryCount < MAX_INIT_RETRIES then
      dlog("D", "lightingController.registerCallbacks", 
          "screenInput.registerLuaCallback not available, retry " .. initRetryCount .. "/" .. MAX_INIT_RETRIES)
      if obj then
        obj:queueLuaCommand("if extensions and extensions.lightingControllerRetry then extensions.lightingControllerRetry() end")
      end
    else
      dlog("E", "lightingController.registerCallbacks", 
          "Failed to register callbacks after " .. MAX_INIT_RETRIES .. " retries")
    end
    return false
  end

  -- Register callbacks directly (like weatherController does)
  screenInput.registerLuaCallback("updateLighting", updateLighting)
  screenInput.registerLuaCallback("getLightingState", getLightingState)
  
  callbacksRegistered = true
  dlog("I", "lightingController.registerCallbacks",
      "Callbacks registered successfully")
  return true
end

-- Retry function for deferred callback registration
-- This is stored globally so queueLuaCommand can access it
local function retryRegisterCallbacks()
  dlog("D", "lightingController.retryRegisterCallbacks", 
      "Retry attempt (count: " .. initRetryCount .. ")")
  registerCallbacks()
end

-- Export the retry function to global extensions table for queueLuaCommand access
if not extensions then extensions = {} end
extensions.lightingControllerRetry = retryRegisterCallbacks

-- ============================================================
-- Lifecycle
-- ============================================================

local function reset(jbeamData)
  dlog('I', 'lightingController.reset', 'lightingController.reset() called')
  buildPidMap()

  -- Load saved lighting config (if any) and apply it
  -- This restores the user's lighting preferences on vehicle spawn/reset
  loadLightingConfig()

  -- BUG FIX #7: Notify JavaScript of vehicle reload to prevent desync
  if screenInput then
    local send = screenInput.pushToScreens or screenInput.callJS
    if send then send('onVehicleReload', {}) end
  end
end

local function init(jbeamData)
  dlog("I", "lightingController.init", 
      "screenInput available: " .. tostring(screenInput ~= nil))
  
  reset(jbeamData)

  -- Attempt to register callbacks immediately
  registerCallbacks()
end

-- ============================================================
-- Module exports
-- ============================================================

M.init      = init
M.reset     = reset
M.updateGFX = updateGFX

-- Individual control
M.setLight      = setLight
M.setBrightness = setBrightness
M.setColor      = setColor
M.turnOn        = turnOn
M.turnOff       = turnOff

-- Bulk control
M.setAll = setAll
M.allOn  = allOn
M.allOff = allOff

-- High-level control (from JavaScript)
M.updateLighting = updateLighting
M.getLightingState = getLightingState

return M