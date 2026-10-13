-- sneppy snep snep! 

-- Be warned (!!!!): using this code means you give  
--    away your soul to the snow leopard gods!

-- so it's like a cool thing that makes it easy to make screens
-- we send coordinates and related DOM events to the js 
-- and that makes development so much easier (win for you). 

local M = {}

M.dependencies = {"ui_imgui"}

local debugLogging = true

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "screenService", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

M.drawBoxes = false

local calibration = require("screenCalibration")

local registeredVehicles = {}
local boxes = {}
local triggers = {}
local vehicle = nil
local referencePlanes = {}
local lastHoveredBoxId = nil
local triggerStates = {}
local refPlaneCache = {}

--------------------------------------------------------------------
-- DRIVING INPUT LOCK while the pointer hovers a screen.
-- Base-game mechanism (core_input_actionFilter, same pattern bigMap /
-- garage / scenario modes use): named game actions stop firing while the
-- lock is on, so keys typed at the screen can't also drive the car, flip
-- cameras or reset it. Camera freelook, menus, pause and the UI keep
-- working on purpose. Extend the list below if another binding bites.
--------------------------------------------------------------------
local drivingBlockActions = {
  -- driving
  "accelerate", "accelerate_brake", "brake",
  "steer_left", "steer_right", "steering",
  "clutch", "shiftUp", "shiftDown", "toggleShifterMode",
  "gear1", "gear2", "gear3", "gear4", "gear5", "gear6", "gear7", "gear8",
  "gearN", "gearR",
  "parkingbrake", "parkingbrake_toggle",
  -- reset / recover / respawn
  "reset_physics", "recover_vehicle", "recover_vehicle_alt",
  "recover_to_last_road", "reload_vehicle",
  -- cameras (digits + C double as typing keys; Tab switches vehicles)
  "toggleCamera", "switch_camera_next",
  "switch_next_vehicle", "switch_previous_vehicle", "switch_next_vehicle_multiseat",
  "camera_1", "camera_10", "camera_2", "camera_3", "camera_4",
  "camera_5", "camera_6", "camera_7", "camera_8", "camera_9",
  -- common-letter collisions
  "couplersToggle", "couplersLock", "couplersUnlock",
  "horn", "highbeam",
  "toggle_headlights", "toggle_foglights",
  "toggle_hazard_signal", "toggle_left_signal", "toggle_right_signal",
  "toggle_lightbar_signal", "toggle_underglow",
  -- fullscreen UI that typing letters would otherwise pop open
  "toggleBigMap", "toggle_minimap",
  "toggleRadialMenuSandbox", "toggleRadialMenuPlayerVehicle",
  "toggleRadialMenuVehicleFeatures", "toggleRadialMenuMulti",
  -- our own actions that share typing keys (space/U/L/Y): while hovering,
  -- the key types into the browser instead of toggling these
  "bbc_media_toggle_media_info",
  "toggle_screen_tuner",
  "bbc_stereo_toggle_speaker_nodes",
  "bbc_stereo_toggle_lighting_props",
}
local drivingBlocked = false
-- Unconditional engine-side clear. The C++ ActionMap filter can outlive Lua
-- reloads/level loads already blocked while fresh Lua believes OFF, so any
-- OFF path must hit the engine even when Lua already reads false.
local function forceScreenDrivingClear(reason)
  drivingBlocked = false
  if core_input_actionFilter then
    core_input_actionFilter.setGroup('bbcScreenDrivingBlock', drivingBlockActions)
    core_input_actionFilter.addAction(0, 'bbcScreenDrivingBlock', false)
  end
  if reason then
    dlog("I", "screenService", "screen driving lock force-cleared (" .. tostring(reason) .. ")")
  end
end
local function setScreenDrivingBlocked(active)
  if active == drivingBlocked then
    -- Lua/C++ desync heal: OFF must still reach the engine. ON is idempotent.
    if not active and core_input_actionFilter then
      core_input_actionFilter.setGroup('bbcScreenDrivingBlock', drivingBlockActions)
      core_input_actionFilter.addAction(0, 'bbcScreenDrivingBlock', false)
    end
    return
  end
  drivingBlocked = active
  dlog("I", "screenService", "screen driving lock " .. (active and "ENGAGED" or "released"))
  if core_input_actionFilter then
    core_input_actionFilter.setGroup('bbcScreenDrivingBlock', drivingBlockActions)
    core_input_actionFilter.addAction(0, 'bbcScreenDrivingBlock', active)
  end
end
local sendHover -- forward declaration (defined below with the other senders)

--------------------------------------------------------------------
-- SCREEN-KEYS ACTION MAP (hover-scoped typing bindings).
-- The 64 bbc_key_* actions live in the "BbcScreenKeys" map (see
-- bbc_screen_keys.json), which the engine creates dormant: its bindings
-- only fire while the map is pushed (same push/pop pattern as the driver
-- camera zoom map). They previously sat in the always-on "First" map,
-- which sits above the vehicle maps and consumed every letter key, so
-- driving died on load. Now: unhovered = popped = keys reach the car;
-- hovered = pushed + driving filter engaged = keys type into the browser.
--------------------------------------------------------------------
local screenKeysMapName = "BbcScreenKeys"
local screenKeysMapPushed = false
local function pushScreenKeysMap()
  if screenKeysMapPushed then return end
  if pushActionMap and pushActionMap(screenKeysMapName) then
    screenKeysMapPushed = true
    dlog("I", "screenService", "screen keys map pushed (typing live)")
  end
end
local function popScreenKeysMap()
  -- Idempotent: safe when absent or not pushed (covers Lua/C++ desync
  -- across extension reloads, where this bool resets but C++ may not).
  screenKeysMapPushed = false
  if popActionMap then
    popActionMap(screenKeysMapName)
  end
end

local function clearScreenInputState()
  -- Called when hover tracking can't run and on vehicle lifecycle events:
  -- never strand the player input-locked or with keys held in the app.
  -- Uses the unconditional clear: Lua may read OFF while C++ is still ON.
  forceScreenDrivingClear(nil)
  popScreenKeysMap()
  lastHoveredBoxId = nil
  if vehicle then
    sendHover(nil)
  end
  if mediaBridgeGE and mediaBridgeGE.releaseAllKeys then
    mediaBridgeGE.releaseAllKeys()
  end
end

-- ROTATION FUNCTIONS (moved to top for calibration code)
local _rotX, _eulerRotX = MatrixF(true), vec3()
local _rotY, _eulerRotY = MatrixF(true), vec3()
local _rotZ, _eulerRotZ = MatrixF(true), vec3()

local function buildRotationMatrix(rot)
    if not rot then
        return MatrixF(true)
    end

    -- Use independent axes
    _eulerRotX:set(math.rad(rot.x or 0), 0, 0)
    _eulerRotY:set(0, math.rad(rot.y or 0), 0)
    _eulerRotZ:set(0, 0, math.rad(rot.z or 0))
    _rotX:setFromEuler(_eulerRotX)
    _rotY:setFromEuler(_eulerRotY)
    _rotZ:setFromEuler(_eulerRotZ)

    local result = _rotX:copy()
    result:mul(_rotY)
    result:mul(_rotZ)
    return result
end

local function defaultRotation(rot)
    if not rot or not rot.x or not rot.y or not rot.z then
        return { x = 0, y = 0, z = 0 }
    end
    return rot
end

-- Rotations are static config data. Build their matrices once at load time
local function precomputeRotationMatrix(rot)
    if not rot or (rot.x == 0 and rot.y == 0 and rot.z == 0) then
        return nil
    end
    return buildRotationMatrix(rot)
end

-- LIVE TUNER API (moved up for calibration code to use)
local function tunerGetState()
    return referencePlanes, boxes, triggers, screenConfigs
end

-- One-shot diagnostics for the click pipeline
local dloggedNoVehicle = false
local dloggedFirstHit = false
local dloggedMissingConfig = {}
local dloggedClickDetails = false

-- Screen configuration storage
local screenConfigs = {}

--------------------------------------------------------------------
-- PER-VEHICLE CALIBRATION (persisted to settings/)
-- Defined early so loadBoxes() can call them
--------------------------------------------------------------------

local function getVehName(vehId)
  local veh = be:getObjectByID(vehId)
  if not veh then return nil end
  if veh.getJBeamFilename then
    local ok, n = pcall(veh.getJBeamFilename, veh)
    if ok and n and n ~= "" then return n end
  end
  return veh.JBeam or veh.jbeam or nil
end

local function applyCalibration(calib)
  if calib.refPlanes then
    for id, p in pairs(calib.refPlanes) do
      if referencePlanes[id] then
        referencePlanes[id].pos = vec3(p.pos.x, p.pos.y, p.pos.z)
        referencePlanes[id].rot = { x = p.rot.x, y = p.rot.y, z = p.rot.z }
        referencePlanes[id].rotMat = precomputeRotationMatrix(referencePlanes[id].rot)
        referencePlanes[id].width = p.width
        referencePlanes[id].height = p.height
      end
    end
  end
  if calib.boxes then
    for i, b in ipairs(calib.boxes) do
      if boxes[i] then
        boxes[i].pos = vec3(b.pos.x, b.pos.y, b.pos.z)
        boxes[i].rot = { x = b.rot.x, y = b.rot.y, z = b.rot.z }
        boxes[i].rotMat = precomputeRotationMatrix(boxes[i].rot)
        if b.size then boxes[i].size = vec3(b.size.x, b.size.y, b.size.z) end
        if b.translateX then boxes[i].translateX = b.translateX end
        if b.translateY then boxes[i].translateY = b.translateY end
      end
    end
  end
  if calib.triggers then
    for i, t in ipairs(calib.triggers) do
      if triggers[i] then
        triggers[i].pos = vec3(t.pos.x, t.pos.y, t.pos.z)
        triggers[i].size = vec3(t.size.x, t.size.y, t.size.z)
        triggers[i].rot = { x = t.rot.x, y = t.rot.y, z = t.rot.z }
        triggers[i].rotMat = precomputeRotationMatrix(triggers[i].rot)
      end
    end
  end
  refPlaneCache = {}
end

local function loadCalibrationForVehicle(vehId)
  local vehName = getVehName(vehId)
  if not vehName then return false end
  local data, source = calibration.load(vehName)
  if not data then
    dlog("I", "screenService", "no saved calibration for vehicle: " .. tostring(vehName))
    return false
  end
  applyCalibration(data)
  dlog("I", "screenService", "loaded " .. source .. " calibration for " .. vehName)
  return true
end

local function saveCalibrationForVehicle(vehId)
  local vehName = getVehName(vehId)
  if not vehName then return false end

  local planes, boxesData, triggersData = tunerGetState()

  local saveData = {
    refPlanes = {},
    boxes = {},
    triggers = {},
  }

  if planes then
    for id, p in pairs(planes) do
      saveData.refPlanes[id] = {
        pos = { x = p.pos.x, y = p.pos.y, z = p.pos.z },
        rot = { x = p.rot.x, y = p.rot.y, z = p.rot.z },
        width = p.width,
        height = p.height,
      }
    end
  end

  if boxesData then
    for _, b in ipairs(boxesData) do
      table.insert(saveData.boxes, {
        pos = { x = b.pos.x, y = b.pos.y, z = b.pos.z },
        rot = { x = b.rot.x, y = b.rot.y, z = b.rot.z },
        size = b.size and { x = b.size.x, y = b.size.y, z = b.size.z } or nil,
        translateX = b.translateX,
        translateY = b.translateY,
      })
    end
  end

  if triggersData then
    for _, t in ipairs(triggersData) do
      table.insert(saveData.triggers, {
        pos = { x = t.pos.x, y = t.pos.y, z = t.pos.z },
        size = { x = t.size.x, y = t.size.y, z = t.size.z },
        rot = { x = t.rot.x, y = t.rot.y, z = t.rot.z },
      })
    end
  end

  return calibration.save(vehName, saveData)
end

local function onVehicleSpawned(vehId)
  loadCalibrationForVehicle(vehId)
  clearScreenInputState()
end

local function onVehicleResetted(vehId)
  loadCalibrationForVehicle(vehId)
  clearScreenInputState()
end

--------------------------------------------------------------------
-- JSON PARSING
--------------------------------------------------------------------

local function parseJSON(path)
    if not path then
        return nil
    end

    local content = readFile(path)
    if not content then
        return nil
    end

    -- Attempt to parse as JSON
    local success, result = pcall(jsonDecode, content)
    if not success then
        dlog("E", "Failed to parse JSON from " .. path .. ": " .. tostring(result))
        return nil
    end
    return result
end

local function findFileBySchema(directory, schemaType)
    -- Find all JSON and JSONC files in directory
    local jsonFiles = FS:findFiles(directory, "*.json", 0, true, false)
    local jsoncFiles = FS:findFiles(directory, "*.jsonc", 0, true, false)

    -- Check JSONC files first 
    -- If you check JSON first, JSONC is never read
    for _, filepath in ipairs(jsoncFiles) do
        local data = parseJSON(filepath)
        if data and data["$configType"] == schemaType then
            return filepath
        end
    end

    -- Check JSON files
    for _, filepath in ipairs(jsonFiles) do
        local data = parseJSON(filepath)
        if data and data["$configType"] == schemaType then
            return filepath
        end
    end

    return nil
end

--------------------------------------------------------------------
-- SCREEN CONFIGURATION
--------------------------------------------------------------------

local function configureScreen(screenId, config)
    screenConfigs[screenId] = {
        width = config.width,
        height = config.height,
        aspectRatio = config.width / config.height
    }
    dlog("I", "screenService", "Screen config registered: " .. tostring(screenId) ..
        " (" .. tostring(config.width) .. "x" .. tostring(config.height) .. ")")
end

--------------------------------------------------------------------
-- COORDINATE TRANSFORMATION
--------------------------------------------------------------------

local _hitVector = vec3()
local function calculateScreenCoordinates(rayHitPos, obb)
    local center = obb:getCenter()
    local halfExt = obb:getHalfExtents()
    _hitVector:setSub2(rayHitPos, center)

    -- Project onto OBB's local axes (X = width, Z = height, Y = depth)
    local localX = _hitVector:dot(obb:getAxis(0)) / halfExt.x
    local localZ = _hitVector:dot(obb:getAxis(2)) / halfExt.z

    -- Convert from [-1, 1] to [0, 1] normalized coordinates
    -- X is inverted: axis 0 points right-to-left, so we flip it
    -- Y is direct: axis 2 already points top-to-bottom correctly
    -- Returned as a tuple
    return clamp(1 - (localX + 1) * 0.5, 0, 1), clamp((localZ + 1) * 0.5, 0, 1)
end

local function normalizedToPixel(normX, normY, width, height)
    return math.floor(normX * width), math.floor(normY * height)
end

--------------------------------------------------------------------
-- EVENT DETECTION
--------------------------------------------------------------------

local function detectMouseEvent()
    local im = ui_imgui

    -- Only process mouse events from left click
    if im.IsMouseDragging(0) then
        return "drag", 0
    end

    if im.IsMouseClicked(0) then
        return "mousedown", 0
    end
    if im.IsMouseReleased(0) then
        return "mouseup", 0
    end

    local mouseWheel = im.GetIO().MouseWheel
    if mouseWheel ~= 0 then
        -- DIAG-WHEEL: proves the physical wheel reached imgui (beamng.log)
        dlog('I', "screenService", "WHEEL detected: " .. tostring(mouseWheel))
        return "wheel", nil, mouseWheel
    end

    return "mousemove", nil

    -- TODO: look back at middle and right click events
    -- TODO: also wouldn't hurt to look at Pointer Events API
end

--------------------------------------------------------------------
-- COMMUNICATION
--------------------------------------------------------------------

-- Escape a string for embedding inside a double quoted Lua string literal
local function escapeLuaString(s)
    return (s:gsub("\\", "\\\\"):gsub('"', '\\"'))
end

-- last mousemove sent to the vehicle; used to skip resends while the cursor is still
local lastMovePixelX = nil
local lastMovePixelY = nil
local lastMoveScreenId = nil
local dragSequenceActive = false
local clickSuppressedByDrag = false
local coordinateEventData = {}
local function sendCoordinateEvent(eventData)
    if not vehicle then
        return
    end

    -- Send coordinate event to all screen controllers
    vehicle:queueLuaCommand([[
        local eventData = lpack.decode("]] .. escapeLuaString(lpack.encode(eventData)) .. [[")

        -- Send to screenInput if it exists (forwards to all screens)
        if screenInput and screenInput.inputCoordinate then
            screenInput.inputCoordinate(eventData)
        end
    ]])
end

local function sendScreenCoordinateEvent(eventType, screenId, x, y, button, pixelX, pixelY, deltaY)
    coordinateEventData.type = eventType
    coordinateEventData.x = x
    coordinateEventData.y = y
    coordinateEventData.screenId = screenId
    coordinateEventData.button = button
    coordinateEventData.pixelX = pixelX
    coordinateEventData.pixelY = pixelY
    coordinateEventData.deltaY = deltaY
    sendCoordinateEvent(coordinateEventData)
end

sendHover = function(boxId)
    if not vehicle then
        return
    end

    vehicle:queueLuaCommand([[
        -- Send hover event to screenInput if it exists
        if screenInput and screenInput.onHover then
            screenInput.onHover(]] .. (boxId and ('"' .. tostring(boxId) .. '"') or 'nil') .. [[)
        end
    ]])
end

local function sendTriggerEvent(eventData)
    if not vehicle then
        return
    end

    vehicle:queueLuaCommand([[
        local eventData = lpack.decode("]] .. escapeLuaString(lpack.encode(eventData)) .. [[")

        -- Send trigger event to screenInput if it exists
        if screenInput and screenInput.onTrigger then
            screenInput.onTrigger(eventData)
        end
    ]])
end

local function detectTriggerInteraction(triggerId)
    local im = ui_imgui
    local state = triggerStates[triggerId] or {
        pressed = false
    }
    local eventData = nil

    if im.IsMouseClicked(0) then
        state.pressed = true
        state.pressTime = os.clock()
        triggerStates[triggerId] = state
        eventData = {
            id = triggerId,
            action = "press"
        }

    elseif im.IsMouseReleased(0) and state.pressed then
        local pressDuration = os.clock() - (state.pressTime or 0)
        state.pressed = false
        triggerStates[triggerId] = state

        if pressDuration > 0.5 then
            eventData = {
                id = triggerId,
                action = "hold",
                duration = pressDuration
            }
        else
            eventData = {
                id = triggerId,
                action = "click"
            }
        end

    elseif im.IsMouseDragging(0) and state.pressed then
        local delta = im.GetMouseDragDelta(0)
        if math.abs(delta.x) > 5 or math.abs(delta.y) > 5 then
            eventData = {
                id = triggerId,
                action = "drag",
                deltaX = delta.x,
                deltaY = delta.y
            }
        end
    end

    return eventData
end

--------------------------------------------------------------------
-- ROTATION FUNCTION
--------------------------------------------------------------------

local _rotX, _eulerRotX = MatrixF(true), vec3()
local _rotY, _eulerRotY = MatrixF(true), vec3()
local _rotZ, _eulerRotZ = MatrixF(true), vec3()

local function buildRotationMatrix(rot)
    if not rot then
        return MatrixF(true)
    end

    -- Use independent axes
    _eulerRotX:set(math.rad(rot.x or 0), 0, 0)
    _eulerRotY:set(0, math.rad(rot.y or 0), 0)
    _eulerRotZ:set(0, 0, math.rad(rot.z or 0))
    _rotX:setFromEuler(_eulerRotX)
    _rotY:setFromEuler(_eulerRotY)
    _rotZ:setFromEuler(_eulerRotZ)

    local result = _rotX:copy()
    result:mul(_rotY)
    result:mul(_rotZ)
    return result
end

local function defaultRotation(rot)
    if not rot or not rot.x or not rot.y or not rot.z then
        return { x = 0, y = 0, z = 0 }
    end
    return rot
end

-- Rotations are static config data. Build their matrices once at load time
local function precomputeRotationMatrix(rot)
    if not rot or (rot.x == 0 and rot.y == 0 and rot.z == 0) then
        return nil
    end
    return buildRotationMatrix(rot)
end

--------------------------------------------------------------------
-- BOX RENDERING
--------------------------------------------------------------------

local function drawBox(box, color)
    local col = color or ColorF(0, 1, 1, 1)
    local center = box:getCenter()
    local halfExt = box:getHalfExtents()
    local cornerA = center + halfExt.x * box:getAxis(0) + halfExt.y * box:getAxis(1) + halfExt.z * box:getAxis(2)
    local cornerB = center + (-halfExt.x) * box:getAxis(0) + halfExt.y * box:getAxis(1) + halfExt.z * box:getAxis(2)
    local cornerC = center + halfExt.x * box:getAxis(0) + (-halfExt.y) * box:getAxis(1) + halfExt.z * box:getAxis(2)
    local cornerD = center + (-halfExt.x) * box:getAxis(0) + (-halfExt.y) * box:getAxis(1) + halfExt.z * box:getAxis(2)
    debugDrawer:drawLine(cornerA, cornerB, col)
    debugDrawer:drawLine(cornerA, cornerC, col)
    debugDrawer:drawLine(cornerC, cornerD, col)
    debugDrawer:drawLine(cornerD, cornerB, col)

    local cornerE = center + halfExt.x * box:getAxis(0) + halfExt.y * box:getAxis(1) + (-halfExt.z) * box:getAxis(2)
    local cornerF = center + (-halfExt.x) * box:getAxis(0) + halfExt.y * box:getAxis(1) + (-halfExt.z) * box:getAxis(2)
    local cornerG = center + halfExt.x * box:getAxis(0) + (-halfExt.y) * box:getAxis(1) + (-halfExt.z) * box:getAxis(2)
    local cornerH = center + (-halfExt.x) * box:getAxis(0) + (-halfExt.y) * box:getAxis(1) + (-halfExt.z) *
                        box:getAxis(2)
    debugDrawer:drawLine(cornerE, cornerF, col)
    debugDrawer:drawLine(cornerE, cornerG, col)
    debugDrawer:drawLine(cornerG, cornerH, col)
    debugDrawer:drawLine(cornerH, cornerF, col)

    debugDrawer:drawLine(cornerA, cornerE, col)
    debugDrawer:drawLine(cornerB, cornerF, col)
    debugDrawer:drawLine(cornerC, cornerG, col)
    debugDrawer:drawLine(cornerD, cornerH, col)
end

local function drawReferencePlane(refPlanePos, combinedVehRefRot, planeRot, vehRot, planeWidth, planeHeight)
    local col = ColorF(1, 1, 1, 1)
    local axisLength = 0.12
    local labelOffset = 0.015
    local arrowSize = 0.008
    local tickSize = 0.005

    -- Position axes are absolute
    local xAxis = vec3(1, 0, 0):rotated(vehRot)
    local yAxis = vec3(0, 1, 0):rotated(vehRot)
    local zAxis = vec3(0, 0, 1):rotated(vehRot)

    -- Translation axes with labels
    local function drawAxisWithLabels(axis, color, axisName, axisLength)
        local posEnd = refPlanePos + axis * axisLength
        local negEnd = refPlanePos - axis * axisLength

        -- Draw positive direction (solid line)
        debugDrawer:drawLine(refPlanePos, posEnd, color)

        -- Draw negative direction (dashed)
        local negSegments = 8
        for i = 1, negSegments do
            local t1 = (i - 1) / negSegments
            local t2 = i / negSegments
            if i % 2 == 1 then
                local p1 = refPlanePos - axis * (axisLength * t1)
                local p2 = refPlanePos - axis * (axisLength * t2)
                debugDrawer:drawLine(p1, p2, ColorF(color.r * 0.6, color.g * 0.6, color.b * 0.6, color.a))
            end
        end

        -- Draw arrow at positive end
        local perp1 = vec3(0, 1, 0):rotated(vehRot)
        local dotProd = perp1.x * axis.x + perp1.y * axis.y + perp1.z * axis.z
        if math.abs(dotProd) > 0.9 then
            perp1 = vec3(0, 0, 1):rotated(vehRot)
            dotProd = perp1.x * axis.x + perp1.y * axis.y + perp1.z * axis.z
        end

        local perp = vec3(perp1.x - dotProd * axis.x, perp1.y - dotProd * axis.y, perp1.z - dotProd * axis.z)
        local perpLen = math.sqrt(perp.x * perp.x + perp.y * perp.y + perp.z * perp.z)
        if perpLen > 0.0001 then
            perp = vec3(perp.x / perpLen, perp.y / perpLen, perp.z / perpLen)
        else
            perp = vec3(1, 0, 0):rotated(vehRot)
        end
        local arrowTip = posEnd
        local arrowBase = posEnd - axis * arrowSize
        local arrowOffset = perp * (arrowSize * 0.5)

        debugDrawer:drawLine(arrowTip, arrowBase + arrowOffset, color)
        debugDrawer:drawLine(arrowTip, arrowBase - arrowOffset, color)

        -- Compute second perpendicular vector
        local perp2 = vec3(axis.y * perp.z - axis.z * perp.y, axis.z * perp.x - axis.x * perp.z,
            axis.x * perp.y - axis.y * perp.x)
        local perp2Len = math.sqrt(perp2.x * perp2.x + perp2.y * perp2.y + perp2.z * perp2.z)
        if perp2Len > 0.0001 then
            perp2 = vec3(perp2.x / perp2Len, perp2.y / perp2Len, perp2.z / perp2Len)
            local arrowOffset2 = perp2 * (arrowSize * 0.5)
            debugDrawer:drawLine(arrowTip, arrowBase + arrowOffset2, color)
            debugDrawer:drawLine(arrowTip, arrowBase - arrowOffset2, color)
        end

        -- Draw tick marks at both ends
        local tickOffset = perp * tickSize
        local tick1 = posEnd + tickOffset
        local tick2 = posEnd - tickOffset
        debugDrawer:drawLine(tick1, tick2, color)

        local tick3 = negEnd + tickOffset
        local tick4 = negEnd - tickOffset
        debugDrawer:drawLine(tick3, tick4, ColorF(color.r * 0.6, color.g * 0.6, color.b * 0.6, color.a))

        -- Draw text labels
        local labelPosPos = posEnd + axis * labelOffset
        local labelPosNeg = negEnd - axis * labelOffset
        if debugDrawer.drawText then
            debugDrawer:drawText(labelPosPos, axisName .. "+", color)
            debugDrawer:drawText(labelPosNeg, axisName .. "-",
                ColorF(color.r * 0.6, color.g * 0.6, color.b * 0.6, color.a))
        elseif debugDrawer.drawString then
            debugDrawer:drawString(labelPosPos, axisName .. "+", color)
            debugDrawer:drawString(labelPosNeg, axisName .. "-",
                ColorF(color.r * 0.6, color.g * 0.6, color.b * 0.6, color.a))
        end
    end

    -- Draw absolute position axes (relative to vehicle rotation only)
    -- These show where the reference plane is positioned in world space
    drawAxisWithLabels(xAxis, ColorF(1, 0, 0, 1), "X", axisLength)
    drawAxisWithLabels(yAxis, ColorF(0, 1, 0, 1), "Y", axisLength)
    drawAxisWithLabels(zAxis, ColorF(0, 0, 1, 1), "Z", axisLength)

    -- Draw relative position axes (relative to reference plane rotation)
    -- These show the coordinate system used by trigger boxes with relative positions
    -- Use slightly shorter length and different style to distinguish from absolute axes
    local relAxisLength = axisLength * 0.85
    local relXAxis = vec3(1, 0, 0):rotated(combinedVehRefRot)
    local relYAxis = vec3(0, 1, 0):rotated(combinedVehRefRot)
    local relZAxis = vec3(0, 0, 1):rotated(combinedVehRefRot)

    -- Draw relative axes with dashed lines and different colors to distinguish
    local function drawRelativeAxis(axis, color, axisName, axisLength)
        local posEnd = refPlanePos + axis * axisLength
        local negEnd = refPlanePos - axis * axisLength

        -- Draw positive direction (dashed line)
        local posSegments = 8
        for i = 1, posSegments do
            local t1 = (i - 1) / posSegments
            local t2 = i / posSegments
            if i % 2 == 1 then
                local p1 = refPlanePos + axis * (axisLength * t1)
                local p2 = refPlanePos + axis * (axisLength * t2)
                debugDrawer:drawLine(p1, p2, color)
            end
        end

        -- Draw negative direction (dotted)
        local negSegments = 8
        for i = 1, negSegments do
            local t1 = (i - 1) / negSegments
            local t2 = i / negSegments
            if i % 3 == 1 then
                local p1 = refPlanePos - axis * (axisLength * t1)
                local p2 = refPlanePos - axis * (axisLength * t2)
                debugDrawer:drawLine(p1, p2, ColorF(color.r * 0.5, color.g * 0.5, color.b * 0.5, color.a))
            end
        end

        -- Draw arrow at positive end (smaller than absolute axes)
        local perp1 = vec3(0, 1, 0):rotated(combinedVehRefRot)
        local dotProd = perp1.x * axis.x + perp1.y * axis.y + perp1.z * axis.z
        if math.abs(dotProd) > 0.9 then
            perp1 = vec3(0, 0, 1):rotated(combinedVehRefRot)
            dotProd = perp1.x * axis.x + perp1.y * axis.y + perp1.z * axis.z
        end

        local perp = vec3(perp1.x - dotProd * axis.x, perp1.y - dotProd * axis.y, perp1.z - dotProd * axis.z)
        local perpLen = math.sqrt(perp.x * perp.x + perp.y * perp.y + perp.z * perp.z)
        if perpLen > 0.0001 then
            perp = vec3(perp.x / perpLen, perp.y / perpLen, perp.z / perpLen)
        else
            perp = vec3(1, 0, 0):rotated(combinedVehRefRot)
        end
        local arrowTip = posEnd
        local arrowBase = posEnd - axis * (arrowSize * 0.8)
        local arrowOffset = perp * (arrowSize * 0.4)

        debugDrawer:drawLine(arrowTip, arrowBase + arrowOffset, color)
        debugDrawer:drawLine(arrowTip, arrowBase - arrowOffset, color)

        -- Draw text labels with "rel" prefix
        local labelPosPos = posEnd + axis * labelOffset
        if debugDrawer.drawText then
            debugDrawer:drawText(labelPosPos, axisName .. "+ (rel)",
                ColorF(color.r * 0.8, color.g * 0.8, color.b * 0.8, color.a))
        elseif debugDrawer.drawString then
            debugDrawer:drawString(labelPosPos, axisName .. "+ (rel)",
                ColorF(color.r * 0.8, color.g * 0.8, color.b * 0.8, color.a))
        end
    end

    -- Draw relative X axis (red, dashed)
    drawRelativeAxis(relXAxis, ColorF(1, 0.3, 0.3, 1), "X", relAxisLength)

    -- Draw relative Y axis (green, dashed)
    drawRelativeAxis(relYAxis, ColorF(0.3, 1, 0.3, 1), "Y", relAxisLength)

    -- Draw relative Z axis (blue, dashed)
    drawRelativeAxis(relZAxis, ColorF(0.3, 0.3, 1, 1), "Z", relAxisLength)

    -- Draw rotation axes
    if planeRot then
        local rotAxisLength = axisLength * 0.8
        local rotRadius = rotAxisLength * 0.3
        local rotArrowSize = 0.006
        local rotCenter = refPlanePos

        -- Rotation axes use combined rotation (vehicle + plane)
        local rotXAxis = vec3(1, 0, 0):rotated(combinedVehRefRot)
        local rotYAxis = vec3(0, 1, 0):rotated(combinedVehRefRot)
        local rotZAxis = vec3(0, 0, 1):rotated(combinedVehRefRot)

        -- Helper function to draw rotation arc with arrow
        local function drawRotationArc(rotAxis, plane1, plane2, color, label, rotValue, reversed)
            -- Draw rotation arc (quarter circle)
            local arcSegments = 20
            for i = 1, arcSegments do
                local angle1 = (i - 1) * (math.pi / 2) / arcSegments
                local angle2 = i * (math.pi / 2) / arcSegments

                local dir1Vec = plane1 * math.cos(angle1) + plane2 * math.sin(angle1)
                local dir1Len = math.sqrt(dir1Vec.x * dir1Vec.x + dir1Vec.y * dir1Vec.y + dir1Vec.z * dir1Vec.z)
                local dir1 = dir1Len > 0.0001 and vec3(dir1Vec.x / dir1Len, dir1Vec.y / dir1Len, dir1Vec.z / dir1Len) or
                                plane1

                local dir2Vec = plane1 * math.cos(angle2) + plane2 * math.sin(angle2)
                local dir2Len = math.sqrt(dir2Vec.x * dir2Vec.x + dir2Vec.y * dir2Vec.y + dir2Vec.z * dir2Vec.z)
                local dir2 = dir2Len > 0.0001 and vec3(dir2Vec.x / dir2Len, dir2Vec.y / dir2Len, dir2Vec.z / dir2Len) or
                                plane1

                local p1 = rotCenter + dir1 * rotRadius
                local p2 = rotCenter + dir2 * rotRadius
                debugDrawer:drawLine(p1, p2, color)
            end

            -- Draw arrow at tip or start of arc
            local arrowAngle = reversed and 0 or math.pi / 2
            local arrowPos = plane1 * math.cos(arrowAngle) + plane2 * math.sin(arrowAngle)
            local arrowPosLen = math.sqrt(arrowPos.x * arrowPos.x + arrowPos.y * arrowPos.y + arrowPos.z * arrowPos.z)
            if arrowPosLen > 0.0001 then
                arrowPos = vec3(arrowPos.x / arrowPosLen, arrowPos.y / arrowPosLen, arrowPos.z / arrowPosLen)
            end

            local tangentDir = -plane1 * math.sin(arrowAngle) + plane2 * math.cos(arrowAngle)
            local tangentLen = math.sqrt(tangentDir.x * tangentDir.x + tangentDir.y * tangentDir.y + tangentDir.z *
                                            tangentDir.z)
            if tangentLen > 0.0001 then
                tangentDir = vec3(tangentDir.x / tangentLen, tangentDir.y / tangentLen, tangentDir.z / tangentLen)
            end

            local arrowTip = rotCenter + arrowPos * rotRadius
            local arrowBase = arrowTip + (reversed and 1 or -1) * tangentDir * rotArrowSize

            local perp = vec3(rotAxis.y * tangentDir.z - rotAxis.z * tangentDir.y,
                rotAxis.z * tangentDir.x - rotAxis.x * tangentDir.z, rotAxis.x * tangentDir.y - rotAxis.y * tangentDir.x)
            local perpLen = math.sqrt(perp.x * perp.x + perp.y * perp.y + perp.z * perp.z)
            if perpLen > 0.0001 then
                perp = vec3(perp.x / perpLen, perp.y / perpLen, perp.z / perpLen)
                local arrowOffset = perp * (rotArrowSize * 0.6)
                debugDrawer:drawLine(arrowTip, arrowBase + arrowOffset, color)
                debugDrawer:drawLine(arrowTip, arrowBase - arrowOffset, color)
            end

            local labelAngle = math.pi / 4
            local labelDir = plane1 * math.cos(labelAngle) + plane2 * math.sin(labelAngle)
            local labelDirLen = math.sqrt(labelDir.x * labelDir.x + labelDir.y * labelDir.y + labelDir.z * labelDir.z)
            if labelDirLen > 0.0001 then
                labelDir = vec3(labelDir.x / labelDirLen, labelDir.y / labelDirLen, labelDir.z / labelDirLen)
            end
            local labelPos = rotCenter + labelDir * (rotRadius * 1.3)

            if debugDrawer.drawText then
                debugDrawer:drawText(labelPos, label, color)
            elseif debugDrawer.drawString then
                debugDrawer:drawString(labelPos, label, color)
            end
        end

        -- Rotation around X axis (pitch) - Cyan
        if planeRot.x then
            local rotXCol = ColorF(0, 1, 1, 0.8)
            drawRotationArc(rotXAxis, rotZAxis, rotYAxis, rotXCol, "RX", planeRot.x, false)
        end

        -- Rotation around Y axis (yaw) - Yellow
        if planeRot.y then
            local rotYCol = ColorF(1, 1, 0, 0.8)
            drawRotationArc(rotYAxis, rotZAxis, rotXAxis, rotYCol, "RY", planeRot.y, true)
        end

        -- Rotation around Z axis (roll) - Magenta
        if planeRot.z then
            local rotZCol = ColorF(1, 0, 1, 0.8)
            drawRotationArc(rotZAxis, rotYAxis, rotXAxis, rotZCol, "RZ", planeRot.z, true)
        end
    end

    -- Draw reference plane outline
    -- Width runs along the plane's local X axis, height along its local Y axis,
    -- so the outline rotates and positions together with the plane transform
    local halfW = (planeWidth or 0.2) * 0.5
    local halfH = (planeHeight or 0.2) * 0.5
    local refXAxis = vec3(1, 0, 0):rotated(combinedVehRefRot)
    local refYAxis = vec3(0, 1, 0):rotated(combinedVehRefRot)
    local corner1 = refPlanePos + refXAxis * halfW + refYAxis * halfH
    local corner2 = refPlanePos + (-refXAxis) * halfW + refYAxis * halfH
    local corner3 = refPlanePos + (-refXAxis) * halfW + (-refYAxis) * halfH
    local corner4 = refPlanePos + refXAxis * halfW + (-refYAxis) * halfH

    debugDrawer:drawLine(corner1, corner2, col)
    debugDrawer:drawLine(corner2, corner3, col)
    debugDrawer:drawLine(corner3, corner4, col)
    debugDrawer:drawLine(corner4, corner1, col)
end

--------------------------------------------------------------------
-- REFERENCE PLANE HANDLING
--------------------------------------------------------------------

local function getReferencePlane(planeId)
    if not referencePlanes or not next(referencePlanes) then
        return nil
    end

    local planeIdStr = planeId and tostring(planeId) or "0"
    return referencePlanes[planeIdStr] or referencePlanes["0"]
end

--------------------------------------------------------------------
-- MAIN UPDATE LOOP
--------------------------------------------------------------------

-- Reusable variables in onUpdate, to reduce GC load
local vehRot = quat()
local vehPos = vec3()
local obb = OrientedBox3F()
local boxPos = vec3()
local _rayHitPos = vec3()
local frameCounter = 0
local vehMat = nil -- refnode matrix for the current frame

-- Per frame combined vehicle + plane transform, cached per reference plane
-- Rotation is composed from the refnode matrix itself
-- frame counter marks whether an entry has been refreshed this frame yet
local function getRefPlaneTransform(cacheKey, refPlane)
    local cached = refPlaneCache[cacheKey]
    if not cached then
        cached = {
            rotQuat = quat(),
            posOffset = vec3(),
            frame = -1
        }
        refPlaneCache[cacheKey] = cached
    end

    if cached.frame ~= frameCounter then
        cached.rotMat = vehMat:copy()
        if refPlane.rotMat then
            cached.rotMat:mul(refPlane.rotMat)
        end
        cached.rotQuat:set(cached.rotMat:toQuatF())
        cached.posOffset:set(refPlane.pos)
        cached.posOffset:setRotate(vehRot)
        cached.posOffset:setAdd(vehPos)
        cached.frame = frameCounter
    end

    return cached
end

-- Build the world-space obb for a box/trigger into the shared obb
local function computeOBB(v)
    local refPlane = getReferencePlane(v.refPlane)
    local baseMat

    if refPlane then
        local cached = getRefPlaneTransform(v.refPlane or "default", refPlane)
        boxPos:set(v.pos)
        boxPos:setRotate(cached.rotQuat)
        boxPos:setAdd(cached.posOffset)
        baseMat = cached.rotMat
    else
        boxPos:set(v.pos)
        boxPos:setRotate(vehRot)
        boxPos:setAdd(vehPos)
        baseMat = vehMat
    end

    local boxMat = baseMat:copy()
    if v.rotMat then
        boxMat:mul(v.rotMat)
    end
    boxMat:setPosition(boxPos)
    obb:set2(boxMat, v.size)
end

local function onUpdate(dt)
    if not vehicle or not vehicle.getPosition or not boxes[1] then
        if not vehicle and boxes[1] and not dloggedNoVehicle then
            dloggedNoVehicle = true
            dlog("W", "screenService", "Trigger boxes loaded but no focused vehicle - screen clicks disabled until setFocusCar (is a player seated?)")
        end
        -- The hover machinery below can't run in this state, so release
        -- everything here: no focused vehicle means the pointer cannot be on
        -- a screen. Without this, leaving/switching vehicles while hovering
        -- strands the driving lock ON (keys appear "blocked" while driving).
        if lastHoveredBoxId ~= nil or drivingBlocked then
            clearScreenInputState()
        end
        return
    end
    if dragSequenceActive and not ui_imgui.IsMouseDown(0) and not ui_imgui.IsMouseReleased(0) then
        dragSequenceActive = false
        clickSuppressedByDrag = false
    end
    local ray = getCameraMouseRay()

    vehMat = vehicle:getRefNodeMatrix()
    vehRot:set(vehMat:toQuatF())
    vehPos:set(vehicle:getPositionXYZ())

    frameCounter = frameCounter + 1

    -- Draw reference planes if debug enabled
    if M.drawBoxes and referencePlanes and next(referencePlanes) then
        for planeId, plane in pairs(referencePlanes) do
            local cached = getRefPlaneTransform(planeId, plane)
            drawReferencePlane(cached.posOffset, cached.rotQuat, plane.rot, vehRot, plane.width, plane.height)
        end
    end

    local currentHoveredBoxId = nil

    for k = 1, #boxes do
        local v = boxes[k]
        if not v.sizeCalculated and v.scale then
            local screenConfig = screenConfigs[v.screenId]
            if screenConfig then
                v.size = vec3(v.scale, v.depth, v.scale / screenConfig.aspectRatio)
                v.sizeCalculated = true
                v.scale = nil
                v.depth = nil
            else
                goto continue
            end
        end

        if not v.size then
            goto continue
        end

        computeOBB(v)

        if M.drawBoxes then
            if v.screenId then
                drawBox(obb, ColorF(1, 0.5, 0, 1))
            else
                drawBox(obb, ColorF(0, 1, 1, 1))
            end
        end

        local dist = intersectsRay_OBB(ray.pos, ray.dir, obb:getCenterHalfExtentAxes())
        if dist < 2 then
            local screenConfig = screenConfigs[v.screenId]
            if screenConfig then
                if not dloggedFirstHit then
                    dloggedFirstHit = true
                    dlog("I", "screenService", "Ray hit screen box '" .. tostring(v.id) ..
                        "' (screenId " .. tostring(v.screenId) .. ") - click pipeline active")
                end
                currentHoveredBoxId = v.id

                _rayHitPos:set(ray.pos.x + ray.dir.x * dist, ray.pos.y + ray.dir.y * dist,
                    ray.pos.z + ray.dir.z * dist)
                local cx, cy = calculateScreenCoordinates(_rayHitPos, obb)
                cx = clamp(cx + v.translateX * (cx - 0.5), 0, 1)
                cy = clamp(cy + v.translateY * (cy - 0.5), 0, 1)
                local eventType, button, mouseWheel = detectMouseEvent()
                local pixelX, pixelY = normalizedToPixel(cx, cy, screenConfig.width, screenConfig.height)

                -- TEMPORARY DEBUG: show where clicks map in the page
                if (eventType == "mousedown" or eventType == "mouseup") and not dloggedClickDetails then
                    dloggedClickDetails = true
                    dlog("I", "screenService", "Click coords: norm=" ..
                        string.format("%.3f,%.3f", cx, cy) ..
                        " -> pixel=" .. tostring(pixelX) .. "," .. tostring(pixelY) ..
                        " (page is " .. tostring(screenConfig.width) .. "x" .. tostring(screenConfig.height) .. ")")
                end

                local sendType = eventType
                local sendClickAfterMouseup = false
                if eventType == "mousedown" then
                    dragSequenceActive = true
                    clickSuppressedByDrag = false
                elseif eventType == "drag" then
                    if not dragSequenceActive then
                        sendScreenCoordinateEvent("mousedown", v.screenId, cx, cy, button or 0, pixelX, pixelY)
                        dragSequenceActive = true
                        clickSuppressedByDrag = false
                    end
                    clickSuppressedByDrag = true
                    sendType = "mousemove"
                elseif eventType == "mouseup" then
                    sendClickAfterMouseup = dragSequenceActive and not clickSuppressedByDrag
                    dragSequenceActive = false
                    clickSuppressedByDrag = false
                end

                -- Skip resending mousemove while the cursor sits still
                local isDuplicateMove = sendType == "mousemove" and pixelX == lastMovePixelX and
                                            pixelY == lastMovePixelY and v.screenId == lastMoveScreenId

                if not isDuplicateMove then
                    sendScreenCoordinateEvent(
                        sendType,
                        v.screenId,
                        cx,
                        cy,
                        button,
                        pixelX,
                        pixelY,
                        eventType == "wheel" and mouseWheel and mouseWheel * -100 or nil
                    )

                    if sendClickAfterMouseup then
                        sendScreenCoordinateEvent("click", v.screenId, cx, cy, button or 0, pixelX, pixelY)
                    end

                    if sendType == "mousemove" then
                        lastMovePixelX, lastMovePixelY, lastMoveScreenId = pixelX, pixelY, v.screenId
                    else
                        -- Clicks can change the DOM under the cursor. Let the next
                        -- mousemove through so the JS hover state refreshes
                        lastMovePixelX, lastMovePixelY, lastMoveScreenId = nil, nil, nil
                    end
                end
            else
                local key = tostring(v.screenId)
                if not dloggedMissingConfig[key] then
                    dloggedMissingConfig[key] = true
                    dlog("W", "screenService", "Ray hit box '" .. tostring(v.id) ..
                        "' but screenId '" .. key .. "' has no registered screen config - events dropped")
                end
            end
        end
        ::continue::
    end

    -- Process triggers
    for k = 1, #triggers do
        local v = triggers[k]

        computeOBB(v)

        if M.drawBoxes then
            drawBox(obb, ColorF(0.5, 0, 1, 1))
        end

        local dist = intersectsRay_OBB(ray.pos, ray.dir, obb:getCenterHalfExtentAxes())

        if dist < 2 and v.id then
            local eventData = detectTriggerInteraction(v.id)
            if eventData then
                sendTriggerEvent(eventData)
            end
        end
    end

    -- Handle hover state changes
    if currentHoveredBoxId ~= lastHoveredBoxId then
        if lastHoveredBoxId then
            -- if mouse left the screen while a drag/press was active, release it
            if dragSequenceActive and lastMovePixelX then
                sendScreenCoordinateEvent("mouseup", lastMoveScreenId, nil, nil, 0, lastMovePixelX, lastMovePixelY)
                dragSequenceActive = false
                clickSuppressedByDrag = false
            end
            sendHover(nil)
        end
        if currentHoveredBoxId then
            sendHover(currentHoveredBoxId)
        end
        lastHoveredBoxId = currentHoveredBoxId
        lastMovePixelX, lastMovePixelY, lastMoveScreenId = nil, nil, nil
        -- Arm/disarm the driving lock with the hover. On leave, also push
        -- key-ups for anything held so no key sticks down in the browser.
        setScreenDrivingBlocked(currentHoveredBoxId ~= nil)
        -- Hover-scope the typing bindings with the same edge: pushed, keys
        -- reach the bbc_key_* actions (driving is filtered); popped, keys
        -- reach the car. The u/l speaker/lighting toggles stay filtered
        -- while hovering so they can't double-fire with typing.
        if currentHoveredBoxId ~= nil then
            pushScreenKeysMap()
        else
            popScreenKeysMap()
        end
        if currentHoveredBoxId == nil and mediaBridgeGE and mediaBridgeGE.releaseAllKeys then
            mediaBridgeGE.releaseAllKeys()
        end
    end
    -- Fail-safe, safe direction only: whenever the pointer is NOT on a
    -- screen, the lock must be off. Heals Lua/engine desync: Lua may read
    -- OFF while the C++ filter stayed ON across a reload (guarded OFF paths
    -- used to early-return without touching the engine). The Lua-var check
    -- heals within one frame; the engine poll (~1Hz) heals C++-only desync.
    if currentHoveredBoxId == nil then
        if drivingBlocked then
            forceScreenDrivingClear("hover-off failsafe")
        elseif core_input_actionFilter and core_input_actionFilter.isActionBlocked
            and frameCounter % 60 == 0 and core_input_actionFilter.isActionBlocked("accelerate") then
            forceScreenDrivingClear("engine still blocked while unhovered")
        end
    end
end

--------------------------------------------------------------------
-- BOX LOADING
--------------------------------------------------------------------

local function parsePlaneData(planeData, filePath)
    local plane = {
        pos = vec3(0, 0, 0),
        rot = {
            x = 0,
            y = 0,
            z = 0
        },
        width = 0.2,
        height = 0.2
    }

    if planeData.pos and planeData.pos.x and planeData.pos.y and planeData.pos.z then
        plane.pos = vec3(planeData.pos.x, planeData.pos.y, planeData.pos.z)
    end

    planeData.rot = defaultRotation(planeData.rot)
    plane.rot = {
        x = planeData.rot.x,
        y = planeData.rot.y,
        z = planeData.rot.z
    }
    plane.rotMat = precomputeRotationMatrix(plane.rot)

    plane.width = planeData.width and tonumber(planeData.width) or 0.2
    plane.height = planeData.height and tonumber(planeData.height) or 0.2

    return plane
end

local function loadBoxes(path)
    local fileData = parseJSON(path)
    if not fileData then
        boxes = {}
        return
    end

    -- Config type check
    local schema = fileData["$configType"]
    if schema and schema ~= "triggerBoxes" then
        dlog("E", "Expected $configType 'triggerBoxes', got '" .. tostring(schema) .. "' in " .. path)
    end

    -- Both formats supported: {"boxes": [...]} or direct array [...]
    if fileData.boxes then
        boxes = fileData.boxes
    elseif fileData[1] then
        boxes = fileData
    else
        dlog("E", "Invalid trigger box file format in " .. path)
        boxes = {}
        return
    end

    -- Load reference planes
    referencePlanes = {}
    if path then
        -- Find reference plane file by config type in same directory
        -- Strip any leading slash: FS:findFiles can return "/vehicles/..." paths,
        -- which fail as search directories
        local basePath = path:match("^(.+)/[^/]+$")
        basePath = basePath and basePath:gsub("^/+", "") or nil
        local refPlanePath = basePath and findFileBySchema(basePath, "referencePlane") or nil

        -- Fallback: try the file directly if the directory search came up empty
        if not refPlanePath and basePath then
            local direct = basePath .. "/referencePlane.jsonc"
            if not FS:fileExists(direct) then
                direct = basePath .. "/referencePlane.json"
            end
            if FS:fileExists(direct) then
                refPlanePath = direct
            else
                dlog("W", "screenService", "loadBoxes: no referencePlane file found in '" .. tostring(basePath) .. "'")
            end
        end

        local refPlaneData = refPlanePath and parseJSON(refPlanePath) or nil
        if refPlaneData then
            -- Config type check
            local schema = refPlaneData["$configType"]
            if schema and schema ~= "referencePlane" then
                dlog("W", "Expected $configType 'referencePlane', got '" .. tostring(schema) .. "' in " .. refPlanePath)
            end

            -- Remove $configType metadata before processing
            refPlaneData["$configType"] = nil

            -- Check for nested "planes" array
            if refPlaneData.planes then
                for i = 1, #refPlaneData.planes do
                    local planeIdStr = refPlaneData.planes[i].id and tostring(refPlaneData.planes[i].id) or
                                            tostring(i - 1)
                    referencePlanes[planeIdStr] = parsePlaneData(refPlaneData.planes[i], refPlanePath)
                end
            elseif refPlaneData.pos or refPlaneData.rot then
                -- Single object format: one reference plane with ID "0"
                referencePlanes["0"] = parsePlaneData(refPlaneData, refPlanePath)
            end
        end
    end

    -- Clean and process boxes
    local cleaned = {}
    for k = 1, #boxes do
        local v = boxes[k]
        if v.pos and v.pos.x and v.pos.y and v.pos.z then
            local box = {
                pos = vec3(v.pos.x, v.pos.y, v.pos.z),
                id = v.id and tostring(v.id) or nil,
                screenId = v.screenId,
                refPlane = v.refPlane and tostring(v.refPlane) or nil
            }

            if v.screenId and v.scale then
                box.scale = v.scale
                box.depth = v.depth or 0.0005
                box.sizeCalculated = false
            elseif v.size and v.size.x and v.size.y and v.size.z then
                box.size = vec3(v.size.x, v.size.y, v.size.z)
                box.sizeCalculated = true
            else
                dlog("E", "Box " .. k .. " missing size data (needs 'screenId'+'scale' or 'size')")
                box = nil
            end

            if box then
                v.rot = defaultRotation(v.rot)
                box.rot = {
                    x = v.rot.x,
                    y = v.rot.y,
                    z = v.rot.z
                }
                box.rotMat = precomputeRotationMatrix(box.rot)
                box.translateX = v.translateX or 0
                box.translateY = v.translateY or 0
                cleaned[#cleaned + 1] = box
            end
        end
    end
    boxes = cleaned

    local planeCount = 0
    for _ in pairs(referencePlanes) do planeCount = planeCount + 1 end
    local parts = {}
    for k = 1, #cleaned do
        parts[#parts + 1] = tostring(cleaned[k].id) .. "->" .. tostring(cleaned[k].screenId)
    end
    dlog("I", "screenService", "loadBoxes: " .. #cleaned .. " boxes (" ..
        table.concat(parts, ", ") .. "), " .. planeCount .. " reference plane(s)")

    -- Load per-vehicle calibration AFTER boxes are loaded (overrides defaults)
    if vehicle then
        loadCalibrationForVehicle(vehicle:getID())
    end
end

local function loadTriggers(path)
    triggerStates = {}  -- Clear old trigger states to prevent memory leak
    
    if not path then
        triggers = {}
        return
    end

    local basePath = path:match("^(.+)/[^/]+$")
    basePath = basePath and basePath:gsub("^/+", "") or nil
    if not basePath then
        triggers = {}
        return
    end

    local triggerPath = findFileBySchema(basePath, "triggers")
    if not triggerPath then
        triggers = {}
        return
    end

    local fileData = parseJSON(triggerPath)
    if not fileData then
        triggers = {}
        return
    end

    local schema = fileData["$configType"]
    if schema and schema ~= "triggers" then
        dlog("W", "Expected $configType 'triggers', got '" .. tostring(schema) .. "' in " .. triggerPath)
    end

    if fileData.triggers then
        triggers = fileData.triggers
    elseif fileData[1] then
        triggers = fileData
    else
        dlog("E", "Invalid trigger file format in " .. triggerPath)
        triggers = {}
        return
    end

    local cleaned = {}
    for k = 1, #triggers do
        local v = triggers[k]
        if v.pos and v.pos.x and v.pos.y and v.pos.z and v.size and v.size.x and v.size.y and v.size.z then
            local trigger = {
                pos = vec3(v.pos.x, v.pos.y, v.pos.z),
                size = vec3(v.size.x, v.size.y, v.size.z),
                id = v.id and tostring(v.id) or nil,
                refPlane = v.refPlane and tostring(v.refPlane) or nil
            }

            v.rot = defaultRotation(v.rot)
            trigger.rot = {
                x = v.rot.x,
                y = v.rot.y,
                z = v.rot.z
            }
            trigger.rotMat = precomputeRotationMatrix(trigger.rot)
            cleaned[#cleaned + 1] = trigger
        end
    end
    triggers = cleaned
end

--------------------------------------------------------------------
-- LIFECYCLE
--------------------------------------------------------------------

-- Queue calls made while vehicle is nil and flush them once setFocusCar establishes the refe
-- callVehicleLua can be called before setFocusCar fires and that causes a whole load of 
-- synchronization issues

local pendingVehicleLuaCalls = {}

local function flushPendingVehicleLuaCalls()
    if not vehicle then return end
    for _, call in ipairs(pendingVehicleLuaCalls) do
        local argsStr = escapeLuaString(lpack.encode(call.args))
        vehicle:queueLuaCommand(string.format('screenInput.onLuaCallback("%s", lpack.decode("%s"))', call.functionName,
            argsStr))
    end
    pendingVehicleLuaCalls = {}
end

local function setFocusCar(id)
    vehicle = scenetree.findObject(id)
    registeredVehicles[id] = true
    dlog("I", "screenService", "setFocusCar: " .. tostring(id) .. " -> " .. tostring(vehicle ~= nil))
    if vehicle then dloggedNoVehicle = false end
    -- New focus starts unhovered: force the engine clear so a stale C++
    -- filter from a previous car/level can't strand driving OFF with Lua OFF.
    forceScreenDrivingClear(nil)
    popScreenKeysMap()
    lastHoveredBoxId = nil
    flushPendingVehicleLuaCalls()
    -- Calibration now loads inside loadBoxes() after boxes are loaded
end

local function onVehicleDestroyed(vid)
    if vehicle and vehicle:getId() == vid then
        vehicle = nil
        screenConfigs = {}
        boxes = {}
        triggers = {}
        referencePlanes = {}
        triggerStates = {}
        refPlaneCache = {}
        lastHoveredBoxId = nil
        dragSequenceActive = false
        clickSuppressedByDrag = false
        lastMovePixelX, lastMovePixelY, lastMoveScreenId = nil, nil, nil
        vehMat = nil
        clearScreenInputState()
    end
    registeredVehicles[vid] = nil
    if not next(registeredVehicles) then
        extensions.unload(M.__extensionName__)
    end
end

local function onExtensionLoaded()
  -- Fresh Lua state, but the C++ input filter can outlive reloads: force the
  -- engine side clear unconditionally here (bypassing the transition guard),
  -- or inputs load in already blocked with Lua believing everything is off.
  forceScreenDrivingClear(nil)
  popScreenKeysMap()
  dlog("I", "screenService", "loaded (input-lock v5): engine filter force-cleared, keys map popped, hover tracking armed")
end

--------------------------------------------------------------------
-- DATA SAVING
--------------------------------------------------------------------
-- Four levels (aka "scopes"):
--   "factory"    - immutable defaults
--   "global"     - defaults for the entire model
--   "identifier" - user account or vehicle
--   "user"       - driver profile within vehicle or account
--
--  "identifier" and "user" are branching - multiple can exist within the same model
--  "factory" and "global" can only have one
--
-- Hierarchy: factory -> global -> identifier -> user
--------------------------------------------------------------------

-- Clean strings for file paths
local function sanitizePathComponent(str)
    if not str or str == "" then
        return "_default"
    end
    local sanitized = str:gsub("[<>:\"/\\|?*%c]", "_")
    sanitized = sanitized:match("^%s*(.-)%s*$")
    if #sanitized > 64 then
        sanitized = sanitized:sub(1, 64)
    end
    if sanitized == "" then
        return "_default"
    end
    return sanitized
end

local function getLicensePlateLocal()
    if not vehicle then
        return nil
    end
    local licensePlate = vehicle.licenseText
    if licensePlate and licensePlate ~= "" then
        return sanitizePathComponent(licensePlate)
    end
    return nil
end

local function getLicensePlate(callbackId)
    if not vehicle then
        return
    end

    local plate = getLicensePlateLocal()
    local packedData = escapeLuaString(lpack.encode(plate))

    vehicle:queueLuaCommand([[
        if screenInput and screenInput.onPersistCallback then
            screenInput.onPersistCallback("plate", "]] .. callbackId .. [[", "]] .. packedData .. [[")
        end
    ]])
end

-- Build file path based on scope
local function buildPersistPath(filename, scope, userId, identifier)
    if not vehicle then
        return nil, nil
    end

    local vehicleModel = vehicle.jbeam or "unknown"
    local baseDir = "settings/persist/" .. vehicleModel

    scope = scope or "global"

    if scope == "factory" then
        local dir = baseDir .. "/_factory"
        return dir .. "/" .. filename .. ".json", dir

    elseif scope == "global" then
        return baseDir .. "/" .. filename .. ".json", baseDir

    elseif scope == "identifier" then
        local id = identifier or getLicensePlateLocal()
        if not id then
            dlog("W", "No identifier or license plate found, returning to global scope")
            return baseDir .. "/" .. filename .. ".json", baseDir
        end
        local sanitizedId = sanitizePathComponent(id)
        local dir = baseDir .. "/" .. sanitizedId
        return dir .. "/" .. filename .. ".json", dir

    elseif scope == "user" then
        local id = identifier or getLicensePlateLocal()
        if not id then
            dlog("W", "No identifier or license plate found, returning to global scope")
            return baseDir .. "/" .. filename .. ".json", baseDir
        end
        local sanitizedId = sanitizePathComponent(id)
        local sanitizedUserId = sanitizePathComponent(userId)
        local dir = baseDir .. "/" .. sanitizedId .. "/user/" .. sanitizedUserId
        return dir .. "/" .. filename .. ".json", dir
    else
        dlog("W", "Unknown scope '" .. tostring(scope) .. "', using global")
        return baseDir .. "/" .. filename .. ".json", baseDir
    end
end

-- Deep merge two tables
local function deepMerge(base, source)
    if type(base) ~= "table" then
        return source
    end
    if type(source) ~= "table" then
        return source
    end

    local result = {}

    for k, v in pairs(base) do
        if type(v) == "table" then
            result[k] = deepMerge({}, v)
        else
            result[k] = v
        end
    end

    for k, v in pairs(source) do
        if type(v) == "table" and type(result[k]) == "table" then
            result[k] = deepMerge(result[k], v)
        else
            result[k] = v
        end
    end

    return result
end

-- Save data
local function persistSave(filename, data, scope, userId, identifier)
    if not vehicle then
        dlog("E", "Cannot save data - no vehicle active")
        return false
    end

    local filePath, dir = buildPersistPath(filename, scope, userId, identifier)
    if not filePath then
        dlog("E", "Cannot build data path")
        return false
    end

    FS:directoryCreate(dir, true)
    jsonWriteFile(filePath, data, true)

    return true
end

-- Internal load data function
local function persistLoadLocal(filename, scope, userId, identifier)
    if not vehicle then
        dlog("E", "Cannot load data - no vehicle active")
        return nil
    end

    local filePath = buildPersistPath(filename, scope, userId, identifier)
    if not filePath then
        dlog("E", "Cannot build data path")
        return nil
    end

    local data = jsonReadFile(filePath)
    return data
end

-- Load data with callback
local function persistLoad(filename, scope, userId, identifier, callbackId)
    if not vehicle then
        dlog("W", "Cannot load data - no vehicle active")
        return
    end

    local data = persistLoadLocal(filename, scope, userId, identifier)

    -- Send to vehicle Lua via screenInput using lpack encoding
    local packedData = escapeLuaString(lpack.encode(data))
    vehicle:queueLuaCommand([[
        if screenInput and screenInput.onPersistLoaded then
            screenInput.onPersistLoaded("]] .. callbackId .. [[", "]] .. packedData .. [[")
        end
    ]])

end

local function persistExistsLocal(filename, scope, userId, identifier)
    if not vehicle then
        return false
    end
    local filePath = buildPersistPath(filename, scope, userId, identifier)
    if not filePath then
        return false
    end
    return FS:fileExists(filePath)
end

local function persistExists(filename, scope, userId, identifier, callbackId)
    if not vehicle then
        return
    end

    local exists = persistExistsLocal(filename, scope, userId, identifier)
    local packedData = escapeLuaString(lpack.encode(exists))

    vehicle:queueLuaCommand([[
        if screenInput and screenInput.onPersistCallback then
            screenInput.onPersistCallback("exists", "]] .. callbackId .. [[", "]] .. packedData .. [[")
        end
    ]])
end

local function persistDelete(filename, scope, userId, identifier)
    if not vehicle then
        dlog("E", "Cannot delete data - no vehicle active")
        return false
    end

    local filePath = buildPersistPath(filename, scope, userId, identifier)
    if not filePath then
        return false
    end

    if FS:fileExists(filePath) then
        FS:removeFile(filePath)
        dlog("I", "Deleted: " .. filePath)
        return true
    end
    return false
end

-- List all user IDs with saved data for this file (internal helper)
local function persistListUsersLocal(filename, identifier)
    if not vehicle then
        return {}
    end

    local vehicleModel = vehicle.jbeam or "unknown"
    local id = identifier or getLicensePlateLocal()
    if not id then
        return {}
    end

    local sanitizedId = sanitizePathComponent(id)
    local userDir = "settings/persist/" .. vehicleModel .. "/" .. sanitizedId .. "/user"
    local users = {}

    local dirs = FS:directoryList(userDir, false, true)
    if dirs then
        for _, dir in ipairs(dirs) do
            local userName = dir:match("([^/\\]+)$")
            if userName then
                local userFile = userDir .. "/" .. userName .. "/" .. filename .. ".json"
                if FS:fileExists(userFile) then
                    table.insert(users, userName)
                end
            end
        end
    end

    return users
end

local function persistListUsers(filename, identifier, callbackId)
    if not vehicle then
        return
    end

    local users = persistListUsersLocal(filename, identifier)
    local packedData = escapeLuaString(lpack.encode(users))

    vehicle:queueLuaCommand([[
        if screenInput and screenInput.onPersistCallback then
            screenInput.onPersistCallback("users", "]] .. callbackId .. [[", "]] .. packedData .. [[")
        end
    ]])
end

--------------------------------------------------------------------
-- FACTORY DEFAULTS
--------------------------------------------------------------------

local factoryDefaults = {}

local function persistRegisterDefaults(filename, defaults)
    if type(defaults) == "table" then
        factoryDefaults[filename] = defaults
    end
end

local function persistInitDefaults(filename, defaults)
    if not vehicle then
        return false
    end

    local filePath, dir = buildPersistPath(filename, "factory")
    if not filePath then
        return false
    end

    if FS:fileExists(filePath) then
        dlog("W", "Factory defaults already exist: " .. filePath)
        return true
    end

    local defaultData = defaults or factoryDefaults[filename] or {}

    if not next(defaultData) then
        dlog("W", "No defaults provided for factory file: " .. filename)
        return false
    end

    FS:directoryCreate(dir, true)
    jsonWriteFile(filePath, defaultData, true)

    return true
end

-- Reset scope to factory defaults
local function persistResetToFactory(filename, scope, userId, identifier)
    if scope == "factory" then
        dlog("E", "Cannot reset factory scope")
        return false
    end
    return persistDelete(filename, scope, userId, identifier)
end

--------------------------------------------------------------------
-- PRESET COPYING
--------------------------------------------------------------------

-- Copy a preset file from vehicle directory to a target scope
-- Mainly used for factory setting presets shipped with the vehicle mod
-- sourcePath: path to preset file relative to vehicle root (e.g., "presets/suspension_defaults.json")
-- filename: target settings filename (without .json extension)
-- targetScope: scope to copy to ("factory", "global", "identifier", or "user")
-- overwrite: if true, overwrites existing file (default: false for safety, but true recommended for factory)
-- userId: user ID (required if targetScope is "user")
-- identifier: custom identifier (overrides license plate for identifier/user scopes)

local function persistCopyPreset(sourcePath, filename, targetScope, overwrite, userId, identifier)
    if not vehicle then
        return false
    end

    -- Build source path (relative to vehicle root)
    local vehicleModel = vehicle.jbeam or "unknown"
    local fullSourcePath = "vehicles/" .. vehicleModel .. "/" .. sourcePath

    -- Check if source file exists
    if not FS:fileExists(fullSourcePath) then
        dlog("E", "Preset file not found: " .. fullSourcePath)
        return false
    end

    -- Read and validate source file
    local presetData = parseJSON(fullSourcePath)
    if not presetData then
        dlog("E", "Failed to parse preset file: " .. fullSourcePath)
        return false
    end

    -- Check if target already exists
    local targetPath, targetDir = buildPersistPath(filename, targetScope, userId, identifier)
    if not targetPath then
        dlog("E", "Cannot build target path for scope: " .. tostring(targetScope))
        return false
    end

    if FS:fileExists(targetPath) and not (overwrite == true) then
        dlog("W", "Target file already exists (use overwrite=true to replace): " .. targetPath)
        return false
    end

    -- Create target directory if needed
    FS:directoryCreate(targetDir, true)

    -- Copy preset data to target location
    local success = jsonWriteFile(targetPath, presetData, true)
    if success then
        return true
    else
        dlog("E", "Failed to write preset to: " .. targetPath)
        return false
    end
end

--------------------------------------------------------------------
-- HIERARCHICAL LOADING
--------------------------------------------------------------------

-- Load and merge data from all scopes in order
local function persistLoadMerged(filename, userId, identifier, callbackId)
    if not vehicle then
        return nil, nil
    end

    local result = {}
    local sources = {}

    local factoryPath = buildPersistPath(filename, "factory", nil, identifier)
    local factoryData = factoryPath and jsonReadFile(factoryPath) or nil

    if not factoryData and factoryDefaults[filename] then
        factoryData = factoryDefaults[filename]
        persistInitDefaults(filename)
    end

    if factoryData then
        result = deepMerge(result, factoryData)
        for k, _ in pairs(factoryData) do
            sources[k] = "factory"
        end
    end

    local globalPath = buildPersistPath(filename, "global", nil, identifier)
    local globalData = globalPath and jsonReadFile(globalPath) or nil
    if globalData then
        result = deepMerge(result, globalData)
        for k, _ in pairs(globalData) do
            sources[k] = "global"
        end
    end

    local id = identifier or getLicensePlateLocal()
    if id then
        local identifierPath = buildPersistPath(filename, "identifier", nil, identifier)
        local identifierData = identifierPath and jsonReadFile(identifierPath) or nil
        if identifierData then
            result = deepMerge(result, identifierData)
            for k, _ in pairs(identifierData) do
                sources[k] = "identifier"
            end
        end

        if userId then
            local userPath = buildPersistPath(filename, "user", userId, identifier)
            local userData = userPath and jsonReadFile(userPath) or nil
            if userData then
                result = deepMerge(result, userData)
                for k, _ in pairs(userData) do
                    sources[k] = "user"
                end
            end
        end
    end

    -- Send result back to vehicle via screenInput if callback requested
    if callbackId and vehicle then
        local packedData = escapeLuaString(lpack.encode(result))
        local packedSources = escapeLuaString(lpack.encode(sources))
        vehicle:queueLuaCommand([[
            if screenInput and screenInput.onPersistMerged then
                screenInput.onPersistMerged("]] .. callbackId .. [[", "]] .. packedData .. [[", "]] .. packedSources ..
                                    [[")
            end
        ]])
    end

    if not next(result) then
        return nil, nil
    end

    return result, sources
end

-- Get which scope a setting came from (internal helper)
local function persistGetSourceLocal(filename, key, userId, identifier)
    local _, sources = persistLoadMerged(filename, userId, identifier)
    if sources then
        return sources[key]
    end
    return nil
end

local function persistGetSource(filename, key, userId, identifier, callbackId)
    if not vehicle then
        return
    end

    local source = persistGetSourceLocal(filename, key, userId, identifier)
    local packedData = escapeLuaString(lpack.encode(source))

    vehicle:queueLuaCommand([[
        if screenInput and screenInput.onPersistCallback then
            screenInput.onPersistCallback("source", "]] .. callbackId .. [[", "]] .. packedData .. [[")
        end
    ]])
end

--------------------------------------------------------------------
-- QUERY API for Individual Scopes
--------------------------------------------------------------------

-- Get a property from a specific scope (not merged)
-- filename: settings file name (without .json)
-- key: property key to query
-- scope: scope to query ("factory", "global", "identifier", or "user")
-- userId: user ID (required if scope is "user")
-- identifier: custom identifier (overrides license plate for identifier/user scopes)
local function persistGetInScope(filename, key, scope, userId, identifier)
    if scope == "factory" then
        -- Check factory file first
        local factoryData = persistLoadLocal(filename, "factory", nil, identifier)
        if factoryData and factoryData[key] ~= nil then
            return factoryData[key]
        end
        -- Fall back to registered factory defaults
        if factoryDefaults[filename] and factoryDefaults[filename][key] ~= nil then
            return factoryDefaults[filename][key]
        end
        return nil
    else
        local data = persistLoadLocal(filename, scope, userId, identifier)
        if data and data[key] ~= nil then
            return data[key]
        end
        return nil
    end
end

--------------------------------------------------------------------
-- QUERY API for Downstream Search
--------------------------------------------------------------------

-- Find a property by searching the hierarchy in order (most specific first)
-- Searches downstream: user -> vehicle -> global -> factory
-- Returns the first match found, along with which scope it came from
-- filename: settings file name (without .json)
-- key: property key to query
-- userId: user ID (includes user scope in search)
-- identifier: custom identifier (overrides license plate)
local function persistFindInHierarchy(filename, key, userId, identifier)
    -- Search order: user -> vehicle -> global -> factory (most specific first)

    -- 1. Check user scope (if userId provided)
    if userId then
        local value = persistGetInScope(filename, key, "user", userId, identifier)
        if value ~= nil then
            return value, "user"
        end
    end

    -- 2. Check identifier scope (if license plate or identifier exists)
    local id = identifier or getLicensePlateLocal()
    if id then
        local value = persistGetInScope(filename, key, "identifier", nil, identifier)
        if value ~= nil then
            return value, "identifier"
        end
    end

    -- 3. Check global scope
    local value = persistGetInScope(filename, key, "global", nil, nil)
    if value ~= nil then
        return value, "global"
    end

    -- 4. Check factory scope (last resort)
    value = persistGetInScope(filename, key, "factory", nil, nil)
    if value ~= nil then
        return value, "factory"
    end

    -- Not found in any scope
    return nil, nil
end

--------------------------------------------------------------------
-- QUERY API for Merged Results
--------------------------------------------------------------------

-- Get any property from merged settings (factory -> global -> identifier -> user)
-- Returns the effective value after all scope merges, or nil if not found
-- This is the final merged value where later scopes override earlier ones
-- For advanced cases (userId, custom identifier), use persistLoadMerged() directly
-- filename: settings file name (without .json)
-- key: property key to query
local function persistGet(filename, key)
    local data, _ = persistLoadMerged(filename, nil, nil)
    if data and data[key] ~= nil then
        return data[key]
    end
    return nil
end

--------------------------------------------------------------------
-- PUBLIC API
--------------------------------------------------------------------

M.onUpdate = onUpdate
M.onExtensionLoaded = onExtensionLoaded
M.onExtensionUnloaded = function() clearScreenInputState() end
M.setFocusCar = setFocusCar
-- True while the pointer hovers any screen box. Key relay (mediaBridgeGE)
-- and the driving lock above both key off this.
M.isScreenHovered = function() return lastHoveredBoxId ~= nil end
M.loadBoxes = loadBoxes
M.loadTriggers = loadTriggers
M.onVehicleDestroyed = onVehicleDestroyed
M.configureScreen = configureScreen -- Allow vehicle controllers to register screen configs

-- Data Persistence
M.persistSave = persistSave
M.persistLoad = persistLoad
M.persistExists = persistExists
M.persistDelete = persistDelete
M.persistListUsers = persistListUsers
M.getLicensePlate = getLicensePlate
M.persistRegisterDefaults = persistRegisterDefaults
M.persistInitDefaults = persistInitDefaults
M.persistResetToFactory = persistResetToFactory
M.persistLoadMerged = persistLoadMerged
M.persistGetSource = persistGetSource
M.persistGet = persistGet
M.persistGetInScope = persistGetInScope
M.persistFindInHierarchy = persistFindInHierarchy
M.persistCopyPreset = persistCopyPreset

local function callVehicleLua(functionName, args)
    if not vehicle then
        table.insert(pendingVehicleLuaCalls, {functionName = functionName, args = args})
        return
    end

    local argsStr = escapeLuaString(lpack.encode(args))
    vehicle:queueLuaCommand(string.format('screenInput.onLuaCallback("%s", lpack.decode("%s"))', functionName,
        argsStr))
end

M.callVehicleLua = callVehicleLua

-- Called by vehicle controller to send lighting prop data to GE
local function onLightingPropsData(data)
    if not data or not data.props then return end
    dlog('I', 'screenService.onLightingPropsData', 'Received ' .. #data.props .. ' lighting props from vehicle controller')
    -- Forward to lightingPropsEditor if loaded
    if extensions and extensions.lightingPropsEditor and extensions.lightingPropsEditor.onLightingPropsData then
        extensions.lightingPropsEditor.onLightingPropsData(data.props)
    else
        dlog('W', 'screenService.onLightingPropsData', 'lightingPropsEditor not loaded or missing onLightingPropsData')
    end
end

M.onLightingPropsData = onLightingPropsData

--------------------------------------------------------------------
-- LIVE TUNER API (used by the screenTuner editor extension)
--------------------------------------------------------------------

local function tunerSetRefPlane(planeId, posTable, rotTable, sizeTable)
    local key = planeId and tostring(planeId) or "0"
    local plane = referencePlanes[key]
    if not plane then return false end
    plane.pos = vec3(posTable.x or 0, posTable.y or 0, posTable.z or 0)
    plane.rot = { x = rotTable.x or 0, y = rotTable.y or 0, z = rotTable.z or 0 }
    plane.rotMat = precomputeRotationMatrix(plane.rot)
    if sizeTable then
        plane.width = sizeTable.width and sizeTable.width > 0 and sizeTable.width or plane.width or 0.2
        plane.height = sizeTable.height and sizeTable.height > 0 and sizeTable.height or plane.height or 0.2
    end
    refPlaneCache[key] = nil
    return true
end

local function tunerSetBox(index, posTable, rotTable, width, translateX, translateY)
    local box = boxes[index]
    if not box then return false end
    box.pos = vec3(posTable.x or 0, posTable.y or 0, posTable.z or 0)
    box.rot = { x = rotTable.x or 0, y = rotTable.y or 0, z = rotTable.z or 0 }
    box.rotMat = precomputeRotationMatrix(box.rot)
    if width and width > 0 and box.size then
        local cfg = box.screenId and screenConfigs[box.screenId]
        box.size = vec3(width, box.size.y, width / (cfg and cfg.aspectRatio or 16 / 9))
    end
    if translateX then box.translateX = translateX end
    if translateY then box.translateY = translateY end
    return true
end

local function tunerSetTrigger(index, posTable, sizeTable, rotTable)
    local trig = triggers[index]
    if not trig then return false end
    trig.pos = vec3(posTable.x or 0, posTable.y or 0, posTable.z or 0)
    if sizeTable then
        trig.size = vec3(sizeTable.x or 0.02, sizeTable.y or 0.02, sizeTable.z or 0.02)
    end
    trig.rot = { x = rotTable.x or 0, y = rotTable.y or 0, z = rotTable.z or 0 }
    trig.rotMat = precomputeRotationMatrix(trig.rot)
    return true
end

M.tunerGetState = tunerGetState
M.tunerSetRefPlane = tunerSetRefPlane
M.tunerSetBox = tunerSetBox
M.tunerSetTrigger = tunerSetTrigger

M.loadCalibrationForVehicle = loadCalibrationForVehicle
M.saveCalibrationForVehicle = saveCalibrationForVehicle
M.onVehicleSpawned = onVehicleSpawned
M.onVehicleResetted = onVehicleResetted

return M

-- mrow~
