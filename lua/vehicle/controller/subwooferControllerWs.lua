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
  dlog('I', "subwooferControllerWs", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

-- Websocket edition of subwooferController.lua. The websocket connection is
-- owned by the GE extension subwooferWsGE (vehicle Lua has no websocket
-- bindings); it relays each JSON payload into this module via the global
-- subwooferWs_receive() (pushed with veh:queueLuaCommand). This controller
-- converts the received sub/bass/mid/treble values into BeamNG electrics
-- values + a voltage simulation. No audio filtering or SFX playback here.
--
-- To use it, point the subwoofer part at this controller in the jbeam:
--   ["subwooferControllerWs", {"waveFrequency": 5.0}]

local json = dofile("lua/vehicle/controller/dkjson.lua")

-- Voltage gauge simulation (RPM-based base voltage + subwoofer sag)
local voltageSim = dofile("lua/vehicle/controller/voltageSim.lua")

-- Audio band values (pushed in over websocket)
local sub_bass = 0
local bass     = 0
local mid      = 0
local treble   = 0

-- Media info from the websocket
local mediaTitle = ""
local mediaArtist = ""
local mediaAlbum = ""
local mediaCurrentTime = 0
local mediaTotalTime = 0
local mediaIsPlaying = false
local mediaThumbnail = ""

-- Local time interpolation for smooth progress bar
local lastServerTime = 0
local lastServerUpdateTime = 0
local lastKnownPlaying = false

-- Voltage (simulated in voltageSim.lua)
local voltage = 12.5

-- Update timing (capped rate for physics tick)
local updateTimer = 0
local UPDATE_INTERVAL = 1 / 288 -- seconds (60Hz max)

-- Elapsed time for data timeouts (physics-clock side)
local simTime = 0
local serverActive = false
local lastServerUpdate = 0
local DATA_TIMEOUT = 5.0 -- seconds without pushed data before fading out

-- Track the last raw payload for debugging
local lastPayloadAt = 0
local receivedCount = 0

-- ==============================
-- Data intake (called from GE via queueLuaCommand)
-- ==============================
function subwooferWs_receive(jsonStr)
  if type(jsonStr) ~= "string" or jsonStr == "" then return end

  local ok, data = pcall(json.decode, jsonStr)
  if not ok or type(data) ~= "table" then
    dlog('W', "subwooferControllerWs", "Received invalid JSON: " .. tostring(data))
    return
  end
  -- tolerate both flat payloads and ones nested under "data"
  if type(data.data) == "table" then data = data.data end

  local hasAudio = data.sub ~= nil or data.bass ~= nil or data.mid ~= nil or data.treble ~= nil
  local hasMedia = data.title ~= nil or data.artist ~= nil or data.album ~= nil
    or data.current_time ~= nil or data.total_time ~= nil or data.is_playing ~= nil
  if not hasAudio and not hasMedia then
    dlog('W', "subwooferControllerWs", "Payload has no audio bands or media info, ignored")
    return
  end

  if not serverActive then
    dlog('I', "subwooferControllerWs", "Websocket data source connected")
  end

  sub_bass = tonumber(data.sub) or 0
  bass     = tonumber(data.bass) or 0
  mid      = tonumber(data.mid) or 0
  treble   = tonumber(data.treble) or 0

  -- Media info (optional fields) - track server time for interpolation
  mediaTitle = data.title or ""
  mediaArtist = data.artist or ""
  mediaAlbum = data.album or ""
  mediaCurrentTime = tonumber(data.current_time) or 0
  mediaTotalTime = tonumber(data.total_time) or 0
  mediaIsPlaying = data.is_playing and true or false
  mediaThumbnail = data.thumbnail or ""

  -- Store server time for local interpolation
  lastServerTime = mediaCurrentTime
  lastServerUpdateTime = simTime
  lastKnownPlaying = mediaIsPlaying

  lastServerUpdate = simTime
  serverActive = true
  lastPayloadAt = simTime
  receivedCount = receivedCount + 1
end

-- ==============================
-- Main physics update
-- ==============================
local function updatePhysics(dt)
  simTime = simTime + dt
  updateTimer = updateTimer + dt
  if updateTimer < UPDATE_INTERVAL then return end
  updateTimer = updateTimer - UPDATE_INTERVAL

  -- Local time interpolation for smooth progress bar
  if mediaIsPlaying and lastKnownPlaying and mediaTotalTime > 0 then
    local elapsed = simTime - lastServerUpdateTime
    if elapsed > 0 and elapsed < 10 then -- Only interpolate for reasonable gaps
      local interpolated = lastServerTime + elapsed
      if interpolated < mediaTotalTime then
        mediaCurrentTime = interpolated
      else
        mediaCurrentTime = mediaTotalTime
      end
    end
  else
    -- Not playing or just started - use server time directly
    mediaCurrentTime = lastServerTime
  end

  -- Source gone quiet - fade out
  if serverActive and simTime - lastServerUpdate > DATA_TIMEOUT then
    serverActive = false
  end

  if not serverActive then
    sub_bass = sub_bass * 0.95
    bass = bass * 0.95
    mid = mid * 0.95
    treble = treble * 0.95

    if sub_bass < 0.01 then sub_bass = 0 end
    if bass < 0.01 then bass = 0 end
    if mid < 0.01 then mid = 0 end
    if treble < 0.01 then treble = 0 end

    -- Clear media info when no source
    if mediaTitle ~= "" or mediaArtist ~= "" or mediaAlbum ~= "" or mediaThumbnail ~= "" then
      mediaTitle = ""
      mediaArtist = ""
      mediaAlbum = ""
      mediaCurrentTime = 0
      mediaTotalTime = 0
      mediaIsPlaying = false
      mediaThumbnail = ""
      dlog('W', "subwooferControllerWs", "No websocket data for " .. DATA_TIMEOUT .. "s - fading out")
    end
  end

  -- Write to BeamNG electrics
  electrics.values.subWoofer = sub_bass
  electrics.values.bass      = bass
  electrics.values.mid       = mid
  electrics.values.treble    = treble

  -- Media info to electrics
  electrics.values.mediaTitle = mediaTitle
  electrics.values.mediaArtist = mediaArtist
  electrics.values.mediaAlbum = mediaAlbum
  electrics.values.mediaCurrentTime = mediaCurrentTime
  electrics.values.mediaTotalTime = mediaTotalTime
  electrics.values.mediaIsPlaying = mediaIsPlaying
  electrics.values.mediaThumbnail = mediaThumbnail

  local subValue = electrics.values.subWoofer or 0

  -- Get RPM
  local rpm = electrics.values.rpm or 0

  -- Voltage gauge simulation
  voltage = voltageSim.update(dt, subValue, rpm)

  electrics.values.volts = voltage
end

-- ==============================
-- Init / Reset
-- ==============================
local function init(jbeamData)
  dlog('I', "subwooferControllerWs", "Loading... (websocket edition, fed by subwooferWsGE)")
  if BNGWSClient then
    -- websocket client only exists in GE lua; presence here just means the
    -- engine build supports it, the actual socket lives in subwooferWsGE
    dlog('I', "subwooferControllerWs", "Initialized with websocket support")
  else
    dlog('I', "subwooferControllerWs", "Initialized - waiting for subwooferWsGE relay")
  end
end

local function reset()
  sub_bass = 0
  bass = 0
  mid = 0
  treble = 0

  mediaTitle = ""
  mediaArtist = ""
  mediaAlbum = ""
  mediaCurrentTime = 0
  mediaTotalTime = 0
  mediaIsPlaying = false
  mediaThumbnail = ""
  lastServerTime = 0
  lastServerUpdateTime = 0
  lastKnownPlaying = false

  -- Voltage
  voltage = 12.5
  voltageSim.reset()
  updateTimer = 0
  simTime = 0

  serverActive = false
  lastServerUpdate = 0
  lastPayloadAt = 0
  receivedCount = 0
end

-- ==============================
-- Exports
-- ==============================
M.init = init
M.reset = reset
M.update = updatePhysics

M.getStatus = function()
  return {
    active = serverActive,
    lastPayloadAt = lastPayloadAt,
    payloadAge = simTime - lastPayloadAt,
    received = receivedCount,
    sub = sub_bass, bass = bass, mid = mid, treble = treble,
    volts = voltageSim.getVoltage(),
  }
end

return M
