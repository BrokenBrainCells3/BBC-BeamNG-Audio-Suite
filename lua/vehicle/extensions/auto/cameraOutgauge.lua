-- Vehicle extension to capture camera data from GE extension
-- Also handles speaker absolute position data from GE extension

local M = {}

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "cameraOutgauge", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

local cameraData = {
  localPos = { x = 0, y = 0, z = 0 },
  localRot = { pitch = 0, yaw = 0, roll = 0 },
  localRotQuat = { x = 0, y = 0, z = 0, w = 0 },
  localForward = { x = 0, y = 0, z = 0 },
  globalPos = { x = 0, y = 0, z = 0 },
  globalRot = { pitch = 0, yaw = 0, roll = 0 },
  globalRotQuat = { x = 0, y = 0, z = 0, w = 0 },
  globalForward = { x = 0, y = 0, z = 0 },
  mode = ""
}

local vehicleData = {
  pos = { x = 0, y = 0, z = 0 },
  rotQuat = { x = 0, y = 0, z = 0, w = 0 }
}

local dataReceived = false
local speakerAbsPositions = {}
local vehicleDataOverride = nil

local function onExtensionLoaded()
  -- The unicycle (walk mode body) must never be tracked/registered.
  if obj.jbeam == "unicycle" then
    dlog('D', "cameraOutgauge", "Skipping unicycle vehicle")
    return
  end
  dlog('I', "cameraOutgauge", "Vehicle extension loaded")
  obj:queueGameEngineLua("extensions.cameraOutgaugeGE.registerVehicle(" .. obj:getID() .. ")")
end

local function onExtensionUnloaded()
  dlog('D', "cameraOutgauge", "Vehicle extension unloaded")
  obj:queueGameEngineLua("extensions.cameraOutgaugeGE.unregisterVehicle(" .. obj:getID() .. ")")
end

-- Update vehicle position and rotation
local function updateVehicleData()
  -- While walking, the GE extension pushes the real car's transform; use it
  -- instead of this vehicle's own (the unicycle) position.
  if vehicleDataOverride then
    vehicleData = vehicleDataOverride
  else
    local vehPos = obj:getPosition()
    local vehRot = quat(obj:getRotation())

    if vehPos then
      vehicleData.pos.x = vehPos.x
      vehicleData.pos.y = vehPos.y
      vehicleData.pos.z = vehPos.z
    end

    if vehRot then
      vehicleData.rotQuat.x = vehRot.x
      vehicleData.rotQuat.y = vehRot.y
      vehicleData.rotQuat.z = vehRot.z
      vehicleData.rotQuat.w = vehRot.w
    end
  end

  -- Store as electrics values for dump(electrics) access
  electrics.values.vehPosX = vehicleData.pos.x or 0
  electrics.values.vehPosY = vehicleData.pos.y or 0
  electrics.values.vehPosZ = vehicleData.pos.z or 0
  electrics.values.vehQuatX = vehicleData.rotQuat.x or 0
  electrics.values.vehQuatY = vehicleData.rotQuat.y or 0
  electrics.values.vehQuatZ = vehicleData.rotQuat.z or 0
  electrics.values.vehQuatW = vehicleData.rotQuat.w or 0
end

-- Set a vehicle transform override (used while walking so the unicycle's
-- outgauge protocol reports the real car's position over UDP).
local function setVehicleData(dataStr)
  local data = jsonDecode(dataStr)
  if not data or not data.pos then return end
  vehicleDataOverride = data
end

-- Called from GE extension with camera data
local function setCameraData(dataStr)
  local data = jsonDecode(dataStr)
  if data then
    cameraData = data
    if not dataReceived then
      dataReceived = true
      dlog('I', "cameraOutgauge", string.format("RECEIVED from GE - Local: (%.2f, %.2f, %.2f) Global: (%.2f, %.2f, %.2f)",
        data.localPos.x, data.localPos.y, data.localPos.z,
        data.globalPos.x, data.globalPos.y, data.globalPos.z))
    end

    -- Store as electrics values for dump(electrics) access
    electrics.values.camLocalX = data.localPos.x or 0
    electrics.values.camLocalY = data.localPos.y or 0
    electrics.values.camLocalZ = data.localPos.z or 0
    electrics.values.camLocalPitch = data.localRot.pitch or 0
    electrics.values.camLocalYaw = data.localRot.yaw or 0
    electrics.values.camLocalRoll = data.localRot.roll or 0
    electrics.values.camLocalQuatX = data.localRotQuat.x or 0
    electrics.values.camLocalQuatY = data.localRotQuat.y or 0
    electrics.values.camLocalQuatZ = data.localRotQuat.z or 0
    electrics.values.camLocalQuatW = data.localRotQuat.w or 0
    electrics.values.camLocalForwardX = data.localForward.x or 0
    electrics.values.camLocalForwardY = data.localForward.y or 0
    electrics.values.camLocalForwardZ = data.localForward.z or 0
    electrics.values.camGlobalX = data.globalPos.x or 0
    electrics.values.camGlobalY = data.globalPos.y or 0
    electrics.values.camGlobalZ = data.globalPos.z or 0
    electrics.values.camGlobalPitch = data.globalRot.pitch or 0
    electrics.values.camGlobalYaw = data.globalRot.yaw or 0
    electrics.values.camGlobalRoll = data.globalRot.roll or 0
    electrics.values.camGlobalQuatX = data.globalRotQuat.x or 0
    electrics.values.camGlobalQuatY = data.globalRotQuat.y or 0
    electrics.values.camGlobalQuatZ = data.globalRotQuat.z or 0
    electrics.values.camGlobalQuatW = data.globalRotQuat.w or 0
    electrics.values.camGlobalForwardX = data.globalForward.x or 0
    electrics.values.camGlobalForwardY = data.globalForward.y or 0
    electrics.values.camGlobalForwardZ = data.globalForward.z or 0
    electrics.values.bbccamMode = data.mode or ""
  end
end

-- Receive absolute positions from GE extension and export to electrics
local function setSpeakerAbsPositions(jsonStr)
  local data = jsonDecode(jsonStr)
  if not data then
    return
  end

  speakerAbsPositions = data

  -- Export to electrics with prefix "speaker_abs_"
  for safeName, pos in pairs(data) do
    electrics.values["speaker_abs_" .. safeName .. "_x"] = pos.x
    electrics.values["speaker_abs_" .. safeName .. "_y"] = pos.y
    electrics.values["speaker_abs_" .. safeName .. "_z"] = pos.z
    electrics.values["speaker_abs_" .. safeName .. "_valid"] = pos.valid and 1 or 0
    electrics.values["speaker_abs_" .. safeName .. "_blocked"] = pos.blocked and 1 or 0
    electrics.values["speaker_abs_" .. safeName .. "_static_blocked"] = pos.staticBlocked and 1 or 0
  end
end

-- UpdateGFX hook - called every frame to keep electric values fresh
local function updateGFX(dt)
  -- Refresh absolute speaker positions from cached data
  for safeName, pos in pairs(speakerAbsPositions) do
    electrics.values["speaker_abs_" .. safeName .. "_x"] = pos.x
    electrics.values["speaker_abs_" .. safeName .. "_y"] = pos.y
    electrics.values["speaker_abs_" .. safeName .. "_z"] = pos.z
    electrics.values["speaker_abs_" .. safeName .. "_valid"] = pos.valid and 1 or 0
    electrics.values["speaker_abs_" .. safeName .. "_blocked"] = pos.blocked and 1 or 0
    electrics.values["speaker_abs_" .. safeName .. "_static_blocked"] = pos.staticBlocked and 1 or 0
  end
end

-- Get camera data (called by protocol)
local function getCameraData()
  if not dataReceived then
    dlog('W', "cameraOutgauge", "Protocol requested data but none received yet!")
  end
  return cameraData
end

M.onExtensionLoaded   = onExtensionLoaded
M.onExtensionUnloaded = onExtensionUnloaded
M.updateGFX           = updateGFX

M.setCameraData       = setCameraData
M.getCameraData       = getCameraData
M.updateVehicleData   = updateVehicleData
M.getVehicleData      = function() return vehicleData end
M.setVehicleData      = setVehicleData

-- Speaker absolute position receiver (from GE extension)
M.setSpeakerAbsPositions = setSpeakerAbsPositions

return M
