-- Game Engine extension to capture real camera position and rotation
-- Also tracks speaker node absolute positions
-- Uses the core_camera API to get actual camera data

local M = {}
M.dependencies = {"gameplay_walk", "ui_imgui"}

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

local speakerCalibration = require("speakerCalibration")

local registeredVehicles = {}
local currentCameraData = nil

-- Walking mode state
local watchedVehicle = nil
local isWalkingMode = false
local speakerLookupBuilt = false

-- Only door coupler states are bridged from the real car to the unicycle while walking.
-- All other electrics (rpm, speed, etc.) come from the unicycle itself naturally.
-- Media electrics are also bridged so the GE extension can read them via core_vehicleBridge.
local bridgedElectrics = {
  "door_FL_coupler_notAttached", "door_FR_coupler_notAttached",
  "door_RL_coupler_notAttached", "door_RR_coupler_notAttached",
  "mediaTitle", "mediaArtist", "mediaAlbum",
  "mediaCurrentTime", "mediaTotalTime", "mediaIsPlaying", "mediaThumbnail",
}

-- Speaker tracking
local speakerNodeIds = {}
local speakerPositionsAbs = {}
local speakerInitialized = false

-- ============================================================
-- ImGui menu config for overriding speaker node names
-- ============================================================
local imgui = ui_imgui

-- Debug flag: when enabled, draws the camera->speaker rays and the vehicle OBB.
-- Default OFF so it doesn't clutter normal driving; toggle in the speaker-node
-- menu, or from console: extensions.cameraOutgaugeGE.setDebugRays(true/false)
local debugRays = false
M.debugRaysPtr = imgui.BoolPtr(debugRays)

local CONFIG_PATH = "settings/bbc_stereo_speaker_nodes.json" -- legacy global path (unused)

-- Sanitize vehicle name for safe file path
local function sanitize(name)
  return tostring(name or ""):gsub("[^%w%-_]", "_")
end

-- Per-vehicle config path
local function getConfigPath(vehName)
  return "settings/bbc_stereo/speaker_nodes/" .. sanitize(vehName) .. ".json"
end

local function getPresetPath(vehName)
  return "mods/unpacked/bbc_stereo_0.6/presets/speaker_nodes/" .. sanitize(vehName) .. ".json"
end

-- Get vehicle JBeam name
local function getVehName(veh)
  if not veh then return nil end
  if veh.getJBeamFilename then
    local ok, n = pcall(veh.getJBeamFilename, veh)
    if ok and n and n ~= "" then return n end
  end
  return veh.JBeam or veh.jbeam or nil
end

-- Shown when the menu is open: whether custom node names override the stock ones.
local useCustomNodes = imgui.BoolPtr(false)
-- Per-role text buffers so users can type a node name that exists on their vehicle.
local customNodeNameBufs = {}
-- Whether the custom/save toggles changed since the last node lookup, so the
-- tracker rebuilds its node->ID table on the next frame.
local forceRebuild = false

-- Whether the node-selection window is open. Toggled by the input action
-- defined in lua/ge/extensions/core/input/actions (Options > Controls), or
-- from console: extensions.cameraOutgaugeGE.toggleSpeakerMenu()
M.windowOpen = imgui.BoolPtr(false)

-- Track last vehicle name for auto-reload when vehicle changes
local lastUIVehName = nil
local uiLoadedForVehicle = false

-- Human-friendly labels for each speaker slot, in the same order as SPEAKER_NODES.
local SPEAKER_LABELS = {
  "Driver Front (FL)",    -- driverspk
  "Driver Rear 1 (RL)",   -- driverrearspk
  "Driver Rear 2 (RL)",   -- driverrear2spk
  "Driver Sub",           -- driversubwoofer
  "Driver Tweeter (Twt L)", -- drivertwt
  "Passenger Front (FR)", -- passengerspk
  "Passenger Rear 1 (RR)", -- passengerrearspk
  "Passenger Rear 2 (RR)", -- passengerrear2spk
  "Passenger Sub",        -- passengersubwoofer
  "Passenger Tweeter (Twt R)", -- passengertwt
}

local SPEAKER_NODES = {
  -- Driver side (left)
  "driverspk",        -- Front door speaker
  "driverrearspk",    -- Rear door speaker 1
  "driverrear2spk",   -- Rear door speaker 2
  "driversubwoofer",  -- Subwoofer
  "drivertwt",        -- Driver tweeter (optional)

  -- Passenger side (right)
  "passengerspk",     -- Front door speaker
  "passengerrearspk", -- Rear door speaker 1
  "passengerrear2spk",-- Rear door speaker 2
  "passengersubwoofer", -- Subwoofer
  "passengertwt",     -- Passenger tweeter (optional)
}

-- Resolve which node name to look up for a role: a custom name wins when the
-- user has enabled custom nodes AND typed something; otherwise fall back to stock.
local function resolveNodeName(speakerName)
  if useCustomNodes[0] then
    local buf = customNodeNameBufs[speakerName]
    if buf and ffi.string(buf) ~= "" then
      return ffi.string(buf)
    end
  end
  return speakerName
end

-- Per-vehicle persist config so choices survive restarts.
local function saveConfigForVehicle(vehName)
  if not vehName then return end
  local data = {
    useCustom = useCustomNodes[0] and true or false,
    names = {}
  }
  for _, name in ipairs(SPEAKER_NODES) do
    local buf = customNodeNameBufs[name]
    data.names[name] = buf and ffi.string(buf) or ""
  end
  local path = getConfigPath(vehName)
  local ok = pcall(jsonWriteFile, path, data, true)
  if not ok then
    dlog('W', "cameraOutgaugeGE", "Failed to write speaker config for " .. vehName)
  else
    dlog('I', "cameraOutgaugeGE", "Saved speaker config for " .. vehName .. " to " .. path)
  end
end

local function loadConfigForVehicle(vehName)
  if not vehName then return end
  local data = jsonReadFile(getConfigPath(vehName))
  if not data then
    -- Try preset
    data = jsonReadFile(getPresetPath(vehName))
    if data then
      dlog('I', "cameraOutgaugeGE", "Loaded preset speaker config for " .. vehName)
    end
  else
    dlog('I', "cameraOutgaugeGE", "Loaded user speaker config for " .. vehName)
  end
  if not data then return end
  useCustomNodes[0] = data.useCustom and true or false
  if data.names then
    for _, name in ipairs(SPEAKER_NODES) do
      local val = data.names[name]
      if val then
        customNodeNameBufs[name] = imgui.ArrayChar(64, val)
      end
    end
  end
  forceRebuild = true
end

-- Per-speaker cache of nearby body triangles (from vdata.triangles). Casting
-- against these each frame gives triangle-accurate line-of-sight, so an open
-- door actually clears the path instead of the coarse whole-vehicle OBB.
local speakerTriangles = {}

-- Reused temp vec3s for per-frame triangle casts (avoid per-frame allocations).
local triV1, triV2, triV3 = vec3(), vec3(), vec3()

-- Game Function Hooks
--------------------------------------------
local function onExtensionLoaded()
  dlog('I', "cameraOutgaugeGE", "GE extension loaded - Using core_camera API")
  
  -- Initialize speaker positions
  for _, speakerName in ipairs(SPEAKER_NODES) do
    speakerPositionsAbs[speakerName] = {
      x = 0,
      y = 0,
      z = 0,
      valid = false
    }
    customNodeNameBufs[speakerName] = imgui.ArrayChar(64, "")
  end
  speakerInitialized = true

  -- Will load per-vehicle config when vehicle spawns/registers
  forceRebuild = true

  -- Also try to load config for currently seated vehicle (handles initial spawn)
  local veh = getPlayerVehicle(0)
  if veh and veh:getJBeamFilename() ~= "unicycle" then
    loadConfigForVehicle(getVehName(veh))
  end
end

local function onExtensionUnloaded()
  dlog('D', "cameraOutgaugeGE", "GE extension unloaded")
  registeredVehicles = {}
  currentCameraData = nil
  speakerNodeIds = {}
  speakerPositionsAbs = {}
  speakerTriangles = {}
  speakerInitialized = false
  watchedVehicle = nil
  isWalkingMode = false
  speakerLookupBuilt = false
end

-- Fired by gameplay_walk just before walking mode is toggled.
-- Captures the real vehicle on the way out, clears it on the way back in.
local function onBeforeWalkingModeToggled(enabled, vehId)
  if enabled then
    local veh = getPlayerVehicle(0)
    if veh and veh:getJBeamFilename() ~= "unicycle" then
      watchedVehicle = veh
      isWalkingMode = true
      speakerLookupBuilt = false
      dlog('I', "cameraOutgaugeGE", "Walking mode ON — tracking vehicle " .. tostring(veh:getId()))
      -- Register for all electrics we want to keep bridging to the unicycle
      for _, key in ipairs(bridgedElectrics) do
        core_vehicleBridge.registerValueChangeNotification(veh, key)
      end
    else
      dlog('I', "cameraOutgaugeGE", "Walking mode ON — but no valid vehicle found to track!")
    end
  else
    dlog('I', "cameraOutgaugeGE", "Walking mode OFF — clearing watched vehicle")
    watchedVehicle = nil
    isWalkingMode = false
    speakerLookupBuilt = false
  end
end

-- Register a vehicle that has the camera extension
local function registerVehicle(vehID)
  local veh = be:getObjectByID(vehID)
  if veh and veh.getJBeamFilename and veh:getJBeamFilename() == "unicycle" then
    dlog('D', "cameraOutgaugeGE", "Ignoring unicycle vehicle registration")
    return
  end
  registeredVehicles[vehID] = true
  dlog('D', "cameraOutgaugeGE", "Vehicle registered: " .. tostring(vehID))
  -- Load per-vehicle speaker config
  if veh then
    loadConfigForVehicle(getVehName(veh))
  end
  -- Register electrics for bridging (door + media)
  reRegisterDoorElectrics(veh)
end

-- Unregister a vehicle
local function unregisterVehicle(vehID)
  registeredVehicles[vehID] = nil
  dlog('D', "cameraOutgaugeGE", "Vehicle unregistered: " .. tostring(vehID))
end

-- Re-register door electrics notifications after a vehicle reset or parts change.
-- We must unregister first — core_vehicleBridge guards against duplicate registrations
-- and will silently bail if the key is already in its table, even after a reset wipes
-- the vlua side. Unregistering clears the GE-side table entry so re-register goes through.
function reRegisterDoorElectrics(veh)
  if not veh then return end
  for _, key in ipairs(bridgedElectrics) do
    core_vehicleBridge.unregisterValueChangeNotification(veh, key)
    core_vehicleBridge.registerValueChangeNotification(veh, key)
  end
end

-- Fired when the player resets their vehicle (F7)
local function onVehicleResetted(vehId)
  local veh = scenetree.findObjectById(vehId)
  if not veh then return end
  -- Ignore the unicycle — it's registered but we never want to bridge its doors
  if veh:getJBeamFilename() == "unicycle" then return end
  if registeredVehicles[vehId] then
    dlog('I', "cameraOutgaugeGE", "Registered vehicle was reset — re-registering door notifications for " .. tostring(vehId))
    reRegisterDoorElectrics(veh)
  end
  if watchedVehicle and watchedVehicle:getId() == vehId then
    dlog('I', "cameraOutgaugeGE", "Watched vehicle was reset — re-registering door notifications")
    reRegisterDoorElectrics(veh)
  end
  -- Reload per-vehicle speaker config after reset
  loadConfigForVehicle(getVehName(veh))
end

-- Fired when a vehicle is spawned
local function onVehicleSpawned(vehId)
  local veh = scenetree.findObjectById(vehId)
  if not veh then return end
  if veh:getJBeamFilename() == "unicycle" then return end
  -- Load per-vehicle speaker config on spawn
  loadConfigForVehicle(getVehName(veh))
end

-- Fired when parts are changed (vehicle gets fully rebuilt, often with a new ID)
local function onVehicleReplaced(oldId, newId)
  -- BeamNG can briefly fire this hook with no replacement ID while the
  -- existing vehicle is being reused/reconfigured. Never index a table
  -- or query the scene tree with a nil ID; the normal register/reset
  -- callbacks will finish the setup immediately afterward.
  if newId == nil then
    dlog(
      'W',
      'cameraOutgaugeGE',
      'Vehicle replacement has no new ID yet; waiting for registration/reset'
    )
    return
  end

  local newVeh = scenetree.findObjectById(newId)
  if newVeh == nil then
    dlog(
      'W',
      'cameraOutgaugeGE',
      'Replacement vehicle is not available yet: ' .. tostring(newId)
    )
    return
  end

  if newVeh:getJBeamFilename() == "unicycle" then return end

  dlog('I', "cameraOutgaugeGE", "Vehicle replaced: " .. tostring(oldId) .. " -> " .. tostring(newId))

  -- Update registeredVehicles to the new ID
  if registeredVehicles[oldId] then
    registeredVehicles[oldId] = nil
    registeredVehicles[newId] = true
  end

  -- If this was our watched vehicle, update the reference and re-register
  if watchedVehicle and watchedVehicle:getId() == oldId then
    watchedVehicle = newVeh
    speakerLookupBuilt = false -- speaker node IDs change with the rebuild
    dlog('I', "cameraOutgaugeGE", "Watched vehicle replaced — updated to " .. tostring(newId))
    reRegisterDoorElectrics(newVeh)
  end
end
-- FIX: use watchedVehicle during walk mode so we look up nodes on the real car,
-- not the unicycle (which has no speaker nodes).
local function buildSpeakerLookup()
  local veh = (isWalkingMode and watchedVehicle) or getPlayerVehicle(0)
  if not veh then
    dlog('W', "cameraOutgaugeGE", "No vehicle available!")
    return false
  end
  
  -- Get vehicle data from core_vehicle_manager
  local vehData = extensions.core_vehicle_manager.getVehicleData(veh:getID())
  if not vehData or not vehData.vdata or not vehData.vdata.nodes then
    dlog('W', "cameraOutgaugeGE", "No vehicle node data available!")
    return false
  end

  -- Clear previous cache
  table.clear(speakerNodeIds)

  local found = 0
  local missing = {}

  for _, speakerName in ipairs(SPEAKER_NODES) do
    local nodeId = nil

    -- Search for node by name in vdata.nodes
    for cid, node in pairs(vehData.vdata.nodes) do
      if node.name == resolveNodeName(speakerName) then
        nodeId = cid
        break
      end
    end

    if nodeId then
      speakerNodeIds[speakerName] = nodeId
      found = found + 1
    else
      table.insert(missing, speakerName)
      -- Zero out this slot so we never export a stale position for a speaker
      -- that isn't actually installed on this vehicle. Without this, switching
      -- from a vehicle with more speakers leaves phantom nodes behind and the
      -- desktop app keeps drawing/panning them.
      speakerPositionsAbs[speakerName] = {
        x = 0,
        y = 0,
        z = 0,
        valid = false
      }
    end
  end

  -- Build a per-speaker cache of nearby body triangles from the vehicle's own
  -- mesh (skipping non-collision triangles, triangleType == 2). Testing these
  -- with live node positions each frame makes the occlusion triangle-accurate,
  -- so an open door stops blocking its speaker.
  table.clear(speakerTriangles)
  local vdata = vehData.vdata
  if vdata.triangles then
    local radius = 2.0
    local radiusSq = radius * radius
    for _, speakerName in ipairs(SPEAKER_NODES) do
      local nodeId = speakerNodeIds[speakerName]
      local node = nodeId and vdata.nodes[nodeId]
      if node then
        local sx, sy, sz = node.pos.x, node.pos.y, node.pos.z
        local tris = {}
        for _, tri in pairs(vdata.triangles) do
          if tri.triangleType ~= 2 then
            local n1, n2, n3 = vdata.nodes[tri.id1], vdata.nodes[tri.id2], vdata.nodes[tri.id3]
            if n1 and n2 and n3 then
              local cx = (n1.pos.x + n2.pos.x + n3.pos.x) / 3
              local cy = (n1.pos.y + n2.pos.y + n3.pos.y) / 3
              local cz = (n1.pos.z + n2.pos.z + n3.pos.z) / 3
              local dx, dy, dz = cx - sx, cy - sy, cz - sz
              if dx * dx + dy * dy + dz * dz < radiusSq then
                table.insert(tris, { tri.id1, tri.id2, tri.id3 })
              end
            end
          end
        end
        speakerTriangles[speakerName] = tris
      end
    end
  end

  return found > 0
end

-- Update speaker absolute positions
-- FIX: use watchedVehicle during walk mode so positions are read from the real car,
-- not the unicycle (which returns nil or wrong positions for these node IDs).
local function updateSpeakerAbsolutePositions()
  local veh = (isWalkingMode and watchedVehicle) or getPlayerVehicle(0)
  if not veh then return end

  for speakerName, nodeId in pairs(speakerNodeIds) do
    -- Get node absolute position in world space
    local absPos = veh:getNodeAbsPosition(nodeId)

    if absPos then
      speakerPositionsAbs[speakerName] = {
        x = absPos.x,
        y = absPos.y,
        z = absPos.z,
        valid = true
      }
    else
      speakerPositionsAbs[speakerName] = {
        x = 0,
        y = 0,
        z = 0,
        valid = false
      }
    end
  end
end

-- Test whether the line from the camera to a speaker is blocked by static
-- geometry (terrain, buildings, static objects) or by the watched vehicle's
-- own body. Static geometry uses a world-space static raycast. The vehicle
-- body uses triangle-accurate casts against the vehicle's own mesh so opening
-- a door actually clears the path. Triangles are always tested — including when
-- the camera sits inside the vehicle's bounding box — so panels between the
-- camera and a speaker always occlude.
-- Returns false (clear), "static" (occluded by a static object), or "body"
-- (occluded by the vehicle's own body).
local function isSpeakerRayBlocked(camPos, speakerPos, speakerName)
  local dx = speakerPos.x - camPos.x
  local dy = speakerPos.y - camPos.y
  local dz = speakerPos.z - camPos.z
  local dist = math.sqrt(dx * dx + dy * dy + dz * dz)
  if dist < 0.05 then return false end

  local camVec = vec3(camPos.x, camPos.y, camPos.z)
  local dirVec = vec3(dx / dist, dy / dist, dz / dist)

  -- Static geometry (terrain, buildings, static objects). Does NOT hit vehicles.
  if castRayStatic then
    local rayDist = castRayStatic(camVec, dirVec, dist)
    if rayDist and rayDist < dist - 0.05 then
      return "static"
    end
  end

  -- Vehicle body. Triangle-accurate against the real mesh (moves with doors).
  local veh = (isWalkingMode and watchedVehicle) or getPlayerVehicle(0)
  if veh and speakerName and speakerTriangles[speakerName] and intersectsRay_Triangle then
    local tris = speakerTriangles[speakerName]
    for i = 1, #tris do
      local t = tris[i]
      -- getNodeAbsPositionXYZ returns a shared/cached vec3, so copy it into
      -- the temp vecs IMMEDIATELY per call (see core/camera.lua pattern).
      triV1:set(veh:getNodeAbsPositionXYZ(t[1]))
      triV2:set(veh:getNodeAbsPositionXYZ(t[2]))
      triV3:set(veh:getNodeAbsPositionXYZ(t[3]))

      -- Cheap pre-filter: triangle centroid must sit between camera and speaker.
      local relX = (triV1.x + triV2.x + triV3.x) / 3 - camVec.x
      local relY = (triV1.y + triV2.y + triV3.y) / 3 - camVec.y
      local relZ = (triV1.z + triV2.z + triV3.z) / 3 - camVec.z
      local along = relX * dirVec.x + relY * dirVec.y + relZ * dirVec.z
      if along > 0 and along < dist then
        local hit = intersectsRay_Triangle(camVec, dirVec, triV1, triV2, triV3)
        if hit and hit > 0.05 and hit < dist - 0.05 then
          return "body"
        end
      end
    end
  end

  return false
end

-- Draw the watched vehicle's world OBB as a wireframe (debug helper).
local function drawVehicleOOBB()
  local veh = (isWalkingMode and watchedVehicle) or getPlayerVehicle(0)
  if not veh or not veh.getSpawnWorldOOBB then return end
  local bb = veh:getSpawnWorldOOBB()
  if not bb or not bb.getAxis then return end

  local c = bb:getCenter()
  local x = bb:getAxis(0) * bb:getHalfExtents().x
  local y = bb:getAxis(1) * bb:getHalfExtents().y
  local z = bb:getAxis(2) * bb:getHalfExtents().z

  -- 8 corners
  local corners = {
    c - x - y - z, c + x - y - z, c + x + y - z, c - x + y - z,
    c - x - y + z, c + x - y + z, c + x + y + z, c - x + y + z,
  }

  -- 12 edges
  local edges = {
    {1,2},{2,3},{3,4},{4,1},
    {5,6},{6,7},{7,8},{8,5},
    {1,5},{2,6},{3,7},{4,8},
  }
  for _, e in ipairs(edges) do
    debugDrawer:drawLine(corners[e[1]], corners[e[2]], ColorF(1, 1, 0, 0.6))
  end
end

-- Compute blocked state for every speaker from the current camera position.
-- Runs every frame in EVERY camera mode (interior, chase, hood, walk, freecam):
-- the camera origin comes from core_camera, so the cast always follows wherever
-- the active camera actually is.
local function computeSpeakerBlockedFlags()
  local camPos = currentCameraData and currentCameraData.globalPos
  if not camPos then return end

  -- Debug visualization: draw the vehicle OBB once per frame.
  if debugRays then
    drawVehicleOOBB()
  end

  for speakerName, nodeId in pairs(speakerNodeIds) do
    local pos = speakerPositionsAbs[speakerName]
    if pos and pos.valid then
      -- pcall so a bad triangle cast can never stop the rays from drawing.
      local ok, blocked = pcall(isSpeakerRayBlocked, camPos, pos, speakerName)
      pos.blocked = ok and (blocked == "static" or blocked == "body") or false
      pos.staticBlocked = ok and blocked == "static" or false

      -- Debug visualization: draw a ray from camera to speaker.
      -- Green = clear line of sight, Yellow = occluded by a static object (echo),
      -- Red = occluded by the vehicle body (muffle).
      -- drawLineInstance with useDepth=false keeps the ray visible even when it
      -- passes through the car body (otherwise it is occluded by the vehicle's
      -- own render meshes when viewed from outside).
      if debugRays then
        local camVec = vec3(camPos.x, camPos.y, camPos.z)
        local spkVec = vec3(pos.x, pos.y, pos.z)
        local col = ColorF(0, 1, 0, 1)
        if ok and blocked == "static" then
          col = ColorF(1, 1, 0, 1)
        elseif ok and blocked == "body" then
          col = ColorF(1, 0, 0, 1)
        end
        debugDrawer:drawLineInstance(camVec, spkVec, 4, col, false)
        debugDrawer:drawSphere(spkVec, 0.08, col)
      end
    elseif pos then
      pos.blocked = false
      pos.staticBlocked = false
    end
  end
end

-- Send absolute positions to vehicle extension (which exports to electrics)
-- During walk mode the player vehicle is the unicycle; deliver to the watched
-- vehicle so the protocol running on the real car keeps receiving positions.
local function exportAbsolutePositionsToElectrics()
  local veh = (isWalkingMode and watchedVehicle) or getPlayerVehicle(0)
  if not veh then
    return
  end

  -- Build position data table
  local posData = {}
  for speakerName, pos in pairs(speakerPositionsAbs) do
    local safeName = speakerName:gsub("[^%w]", "_")
    posData[safeName] = {
      x = pos.x,
      y = pos.y,
      z = pos.z,
      valid = pos.valid,
      blocked = pos.blocked and true or false,
      staticBlocked = pos.staticBlocked and true or false
    }
  end

  -- Send to vehicle extension to export as electric values
  local cmd = "if extensions.cameraOutgauge then extensions.cameraOutgauge.setSpeakerAbsPositions('" .. jsonEncode(posData) .. "') end"
  veh:queueLuaCommand(cmd)

  -- Also push speaker positions into the unicycle while walking
  if isWalkingMode then
    local unicycle = gameplay_walk.getCurrentUnicycle()
    if unicycle then unicycle:queueLuaCommand(cmd) end
  end
end

-- Get camera data directly from core_camera
local function getCameraData()
  -- Initialize data structure
  currentCameraData = {
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

  -- Get camera mode
  currentCameraData.mode = core_camera.getActiveCamName(0) or ""

  -- Get camera position
  local camPos = core_camera.getPosition()
  if camPos then
    currentCameraData.globalPos.x = camPos.x
    currentCameraData.globalPos.y = camPos.y
    currentCameraData.globalPos.z = camPos.z
  end

  -- Get camera rotation (quaternion) and forward direction
  local camQuat = core_camera.getQuat()
  local camForward = core_camera.getForward()
  if camQuat then
    -- Store quaternion directly (xyzw format)
    currentCameraData.globalRotQuat.x = camQuat.x or 0
    currentCameraData.globalRotQuat.y = camQuat.y or 0
    currentCameraData.globalRotQuat.z = camQuat.z or 0
    currentCameraData.globalRotQuat.w = camQuat.w or 0

    -- Also convert to Euler angles for backwards compatibility
    local x, y, z, w = camQuat.x, camQuat.y, camQuat.z, camQuat.w

    -- Roll (x-axis rotation)
    local sinr_cosp = 2 * (w * x + y * z)
    local cosr_cosp = 1 - 2 * (x * x + y * y)
    currentCameraData.globalRot.roll = math.atan2(sinr_cosp, cosr_cosp)

    -- Pitch (y-axis rotation)
    local sinp = 2 * (w * y - z * x)
    if math.abs(sinp) >= 1 then
      currentCameraData.globalRot.pitch = math.copysign(math.pi / 2, sinp)
    else
      currentCameraData.globalRot.pitch = math.asin(sinp)
    end

    -- Yaw (z-axis rotation)
    local siny_cosp = 2 * (w * z + x * y)
    local cosy_cosp = 1 - 2 * (y * y + z * z)
    currentCameraData.globalRot.yaw = math.atan2(siny_cosp, cosy_cosp)
  end

  -- Store forward direction
  if camForward then
    currentCameraData.globalForward.x = camForward.x or 0
    currentCameraData.globalForward.y = camForward.y or 0
    currentCameraData.globalForward.z = camForward.z or 0
  end

  -- Try to get local camera data relative to the reference vehicle.
  -- During walk mode the player vehicle is the unicycle, so use the watched
  -- (real) vehicle as the reference to keep local coords tied to the car.
  local playerVeh = be:getPlayerVehicle(0)
  local refVeh = (isWalkingMode and watchedVehicle) or playerVeh
  if refVeh then
    local vehPos = refVeh:getPosition()

    if vehPos and camPos then
      -- Calculate local position (camera relative to vehicle)
      currentCameraData.localPos.x = camPos.x - vehPos.x
      currentCameraData.localPos.y = camPos.y - vehPos.y
      currentCameraData.localPos.z = camPos.z - vehPos.z
    end

    -- Local rotation is same as global for free camera
    currentCameraData.localRot.pitch = currentCameraData.globalRot.pitch
    currentCameraData.localRot.yaw = currentCameraData.globalRot.yaw
    currentCameraData.localRot.roll = currentCameraData.globalRot.roll
    currentCameraData.localRotQuat.x = currentCameraData.globalRotQuat.x
    currentCameraData.localRotQuat.y = currentCameraData.globalRotQuat.y
    currentCameraData.localRotQuat.z = currentCameraData.globalRotQuat.z
    currentCameraData.localRotQuat.w = currentCameraData.globalRotQuat.w
    currentCameraData.localForward.x = currentCameraData.globalForward.x
    currentCameraData.localForward.y = currentCameraData.globalForward.y
    currentCameraData.localForward.z = currentCameraData.globalForward.z
  end

  -- Deliver camera data to the vehicle running the protocol. During walk mode
  -- that's the watched vehicle plus the unicycle (the unicycle's protocol is the
  -- one that can actually send, since the player is seated in it).
  local targetVeh
  if isWalkingMode then
    targetVeh = watchedVehicle
  elseif playerVeh and registeredVehicles[playerVeh:getID()] then
    targetVeh = playerVeh
  end
  if targetVeh then
    targetVeh:queueLuaCommand("if extensions.cameraOutgauge then extensions.cameraOutgauge.setCameraData('" .. jsonEncode(currentCameraData) .. "') end")
  end

  -- Mirror camera data into the unicycle while walking so its outgauge protocol
  -- has the data it needs to send over UDP.
  if isWalkingMode then
    local unicycle = gameplay_walk.getCurrentUnicycle()
    if unicycle and unicycle ~= targetVeh then
      unicycle:queueLuaCommand("if extensions.cameraOutgauge then extensions.cameraOutgauge.setCameraData('" .. jsonEncode(currentCameraData) .. "') end")
    end
  end

  return currentCameraData
end

-- Push cached electrics from the real car into the unicycle every frame while walking
local function forwardElectricsToUnicycle()
  if not isWalkingMode or not watchedVehicle then return end
  local unicycle = gameplay_walk.getCurrentUnicycle()
  if not unicycle then return end

  local vehId = watchedVehicle:getId()
  local data = {}
  for _, key in ipairs(bridgedElectrics) do
    local val = core_vehicleBridge.getCachedVehicleData(vehId, key)
    if val ~= nil then
      data[key] = val
    end
  end

  if next(data) then
    local encoded = jsonEncode(data)
    unicycle:queueLuaCommand(
      "local d = jsonDecode('" .. encoded .. "') " ..
      "if d then for k,v in pairs(d) do electrics.values[k] = v end end"
    )
  end
end

-- Push the watched (real) vehicle transform into the unicycle while walking so the
-- unicycle's outgauge protocol sends the real car's position over UDP.
local function forwardVehicleDataToUnicycle()
  if not isWalkingMode or not watchedVehicle then return end
  local unicycle = gameplay_walk.getCurrentUnicycle()
  if not unicycle then return end

  local pos = watchedVehicle:getPosition()
  if not pos then return end
  local rot = quat(watchedVehicle:getRotation())

  local data = {
    pos = { x = pos.x, y = pos.y, z = pos.z },
    rotQuat = { x = rot.x, y = rot.y, z = rot.z, w = rot.w }
  }
  unicycle:queueLuaCommand(
    "if extensions.cameraOutgauge then extensions.cameraOutgauge.setVehicleData('" .. jsonEncode(data) .. "') end"
  )
end

-- ============================================================
-- ImGui window
-- ============================================================
local function drawSpeakerWindow()
  imgui.SetNextWindowSize(imgui.ImVec2(380, 0), imgui.Cond_FirstUseEver)

  if not imgui.Begin("BBC Speaker Nodes", nil, imgui.WindowFlags_NoCollapse) then
    imgui.End()
    return
  end

  -- Auto-load config for current vehicle when window opens or vehicle changes
  local currentVeh = getPlayerVehicle(0)
  local currentVehName = currentVeh and getVehName(currentVeh)
  if currentVehName ~= lastUIVehName then
    lastUIVehName = currentVehName
    uiLoadedForVehicle = false
  end
  if not uiLoadedForVehicle and currentVehName then
    loadConfigForVehicle(currentVehName)
    uiLoadedForVehicle = true
  end

  imgui.TextWrapped("Assign node names that exist on your vehicle so the audio " ..
    "visualizer reports their positions over UDP.")

  imgui.Spacing()
  -- Stock vs custom master toggle (default stock).
  if imgui.Checkbox("Use custom node names", useCustomNodes) then
    forceRebuild = true
  end
  imgui.SameLine()
  imgui.TextColored(imgui.ImVec4(0.6, 0.6, 1, 1), "(stock is always default)")

  if useCustomNodes[0] then
    imgui.Spacing()
    imgui.Separator()
    imgui.TextWrapped("For each speaker slot, type the exact node name. " ..
      "Blank slots fall back to the stock name.")

    -- Per-slot text fields.
    imgui.Spacing()
    for i, speakerName in ipairs(SPEAKER_NODES) do
      local label = SPEAKER_LABELS[i] or speakerName
      local buf = customNodeNameBufs[speakerName] or ""
      if imgui.InputText(label .. "##" .. speakerName, buf, 64) then
        forceRebuild = true
      end
    end

    imgui.Spacing()
    if imgui.Button("Save") then
      if currentVehName then saveConfigForVehicle(currentVehName) end
    end
    imgui.SameLine()
    if imgui.Button("Load") then
      if currentVehName then loadConfigForVehicle(currentVehName) end
    end
    imgui.SameLine()
    if imgui.Button("Reset to stock") then
      for _, name in ipairs(SPEAKER_NODES) do
        customNodeNameBufs[name] = imgui.ArrayChar(64, "")
      end
      useCustomNodes[0] = false
      forceRebuild = true
      if currentVehName then saveConfigForVehicle(currentVehName) end
    end
  end

  imgui.Spacing()
  imgui.TextWrapped("Vehicles that already have the stock speaker nodes work automatically.")

  imgui.Spacing()
  imgui.Separator()
  if imgui.Checkbox("Debug visualization (camera rays + vehicle box)", M.debugRaysPtr) then
    debugRays = M.debugRaysPtr[0] and true or false
  end

  imgui.End()
end

-- Update hook to update camera and speaker data every frame
local function onUpdate(dtReal, dtSim, dtRaw)
  -- Draw the menu when open. Toggle via input action (Options > Controls),
  -- or from console: extensions.cameraOutgaugeGE.toggleSpeakerMenu()
  if M.windowOpen[0] then
    drawSpeakerWindow()
  end

  -- Update camera data each frame for smooth output
  getCameraData()

  -- Forward real car electrics into unicycle while walking
  forwardElectricsToUnicycle()

  -- Forward the real car's position into the unicycle while walking
  forwardVehicleDataToUnicycle()

  -- Update speaker positions
  if speakerInitialized then
    -- speakerNodeIds is a dict so we can't use #, use a boolean flag instead
    if not speakerLookupBuilt or forceRebuild then
      buildSpeakerLookup()
      speakerLookupBuilt = true
      forceRebuild = false
    end

    updateSpeakerAbsolutePositions()
    computeSpeakerBlockedFlags()
    exportAbsolutePositionsToElectrics()
  end
end

-- Enable/disable debug ray visualization.
-- Callable from console: extensions.cameraOutgaugeGE.setDebugRays(true)
local function setDebugRays(enabled)
  debugRays = enabled and true or false
  M.debugRaysPtr[0] = debugRays
  dlog('I', "cameraOutgaugeGE", "Debug rays " .. (debugRays and "ENABLED" or "disabled"))
end

-- Enable/disable debug logging.
-- Callable from console: extensions.cameraOutgaugeGE.setDebugLogging(true)
local function setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "cameraOutgaugeGE", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

-- Export Interface
--------------------------------------------
M.onExtensionLoaded          = onExtensionLoaded
M.onExtensionUnloaded        = onExtensionUnloaded
M.onUpdate                   = onUpdate
M.onBeforeWalkingModeToggled = onBeforeWalkingModeToggled
M.onVehicleResetted          = onVehicleResetted
M.onVehicleSpawned           = onVehicleSpawned
M.onVehicleReplaced          = onVehicleReplaced

M.registerVehicle            = registerVehicle
M.unregisterVehicle          = unregisterVehicle
M.getCameraData              = getCameraData
M.setDebugRays               = setDebugRays
M.setDebugLogging            = setDebugLogging
M.toggleSpeakerMenu          = function() M.windowOpen[0] = not M.windowOpen[0] end
M.setSpeakerMenuOpen         = function(open) M.windowOpen[0] = open and true or false end

-- Per-vehicle speaker config
M.saveSpeakerConfig          = saveConfigForVehicle
M.loadSpeakerConfig          = loadConfigForVehicle
M.getVehName                 = getVehName

return M