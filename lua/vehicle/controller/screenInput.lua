-- sneppy snep snep! 

-- Be warned (!!!!): using this code means you give  
--    away your soul to the snow leopard gods!

-- the bridge between the boxes/trigger data and the screen
-- sends events and coordinates to the javascript for it to handle
-- execution is still lua-side though

local M = {}

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "screenInput", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

local gaugeHTMLTextures = {}
local serviceExtension = ""
local triggerConfigPath = ""
local drawBoxes = false

local playerSeated = false

-- Screen lock: when true, all coordinate input to the screen webview is
-- dropped (toggled by the physical "screen_lock" trigger volume)
local screenLocked = false

-- Flag to track if HTML is ready
local htmlReady = false

-- Lua callback handlers (simple table: functionName -> handler)
local luaCallbackHandlers = {}

-- Send JS calls to all registered HTML screens
local function callJS(...)
    if not htmlReady then
        dlog('D', "screenInput.callJS", "Screen not ready, queueing call: " .. tostring(...))
        return
    end
    for i = 1, #gaugeHTMLTextures do
        gaugeHTMLTextures[i]:callJS(...)
    end
end

-- Queue for ignition events before screen is ready
local queuedIgnitionEvents = {}

-- Queue an ignition event to be sent when screen is ready
local function queueIgnitionEvent(fnName, data)
    if htmlReady then
        callJS(fnName, data)
    else
        table.insert(queuedIgnitionEvents, { fnName = fnName, data = data })
        dlog('D', "screenInput", "Queued ignition event: " .. fnName)
    end
end

local function onPlayersChanged(active)
    dlog('I', "screenInput", "Player seated changed: " .. tostring(active) .. " - registering with " .. serviceExtension)
    if active then
        obj:queueGameEngineLua([[
            if ]] .. serviceExtension .. [[ then
                ]] .. serviceExtension .. [[.setFocusCar(]] .. objectId .. [[)
                ]] .. serviceExtension .. [[.drawBoxes = ]] .. tostring(drawBoxes) .. [[
            end
        ]])
    end
end

-- Called by HTML when it's ready
local function onHTMLReady()
    htmlReady = true
    dlog('I', "screenInput", "HTML screen is ready")
    
    -- Process any queued ignition events
    for _, event in ipairs(queuedIgnitionEvents) do
        callJS(event.fnName, event.data)
    end
    queuedIgnitionEvents = {}
end

local function updateGFX(dt)
    if playerInfo.anyPlayerSeated ~= playerSeated then
        onPlayersChanged(playerInfo.anyPlayerSeated)
        playerSeated = playerInfo.anyPlayerSeated
    end

    -- Media info is pushed to the screen from GE mediaBridgeGE
    -- (vehicle Lua cannot index GE extensions directly)
end

local screenReadyCallbacks = {}

local function addScreen(htmlTexture)
    gaugeHTMLTextures[#gaugeHTMLTextures + 1] = htmlTexture
    -- Notify any waiting controllers that screen is ready
    for _, cb in ipairs(screenReadyCallbacks) do
        cb()
    end
    screenReadyCallbacks = {}
end

-- Allow other controllers to register for screen-ready notification
local function onScreenReady(callback)
    if #gaugeHTMLTextures > 0 then
        callback() -- Already ready
    else
        table.insert(screenReadyCallbacks, callback)
    end
end

local function initLastStage()
    -- Load trigger boxes from configured path
    -- loadTriggers() will find the triggers file in the same directory
    if not triggerConfigPath or triggerConfigPath == "" then
        return -- No path configured, skip loading
    end

    obj:queueGameEngineLua([[
        if ]] .. serviceExtension .. [[ then
            local basePath = "]] .. triggerConfigPath .. [["
            local files = FS:findFiles(basePath, "*.json", 0, true, false)
            local jsoncFiles = FS:findFiles(basePath, "*.jsonc", 0, true, false)
            
            -- Combine json and jsonc files
            for _, file in ipairs(jsoncFiles) do
                table.insert(files, file)
            end

            -- Find the triggerBoxes file
            for _, filepath in ipairs(files) do
                local content = readFile(filepath)
                if content and content:match('"$configType"%s*:%s*"triggerBoxes"') then
                    -- Load trigger boxes (screen interaction areas)
                    ]] .. serviceExtension .. [[.loadBoxes(filepath)
                    -- Load triggers (physical volumes) - searches same directory
                    ]] .. serviceExtension .. [[.loadTriggers(filepath)
                    -- Calibration now loads automatically inside loadBoxes()
                    break
                end
            end
        end
    ]])
end

local function reset()
    -- Reset handled by screenService
end

local function init(jbeamData)
    dlog('I', 'screenInput.init', 'screenInput.init() called, registering as global')
    serviceExtension = "screenService"

    -- Configure trigger config path from jbeam (optional)
    -- Defaults to vehicles/common/BBC_Universal_Screen/ if not specified
    if jbeamData and jbeamData.triggerConfigPath then
        triggerConfigPath = jbeamData.triggerConfigPath
    else
        triggerConfigPath = "vehicles/common/BBC_Universal_Screen/"
    end

    drawBoxes = jbeamData and jbeamData.drawBoxes or false

    -- Reload the screenService extension
    obj:queueGameEngineLua("extensions.reload('screenService')")

    -- Load lighting calibration extension for per-vehicle lighting persistence
    obj:queueGameEngineLua("extensions.load('lightingCalibration')")

    -- Make sure the GE media bridge is loaded
    -- (modscript.lua only runs on mod activation and can be skipped)
    obj:queueGameEngineLua("if not mediaBridgeGE then extensions.load('mediaBridgeGE') end")

    -- Load the maps extension
    obj:queueGameEngineLua("extensions.load('lnequMaps')")

    -- Handler for setting electrics values from HTML (e.g. GPS toggle)
    M.registerLuaCallback("setElectrics", function(args)
        if not args or not args.values then return end
        local veh = be and be.getPlayerVehicle and be:getPlayerVehicle(0) or nil
        if veh then
            for k, v in pairs(args.values) do
                veh:queueLuaCommand('electrics.values.' .. k .. ' = ' .. tostring(v))
            end
        end
    end)

    -- Relay taps on the wsstream video feed to the external app so it can
    -- drive the embedded Opera window (normalized 0..1 coords from JS).
    M.registerLuaCallback("wsClick", function(args)
        if not args or args.x == nil or args.y == nil then return end
        local x = math.max(0, math.min(1, tonumber(args.x) or 0))
        local y = math.max(0, math.min(1, tonumber(args.y) or 0))
        obj:queueGameEngineLua(string.format("if mediaBridgeGE and mediaBridgeGE.sendClick then mediaBridgeGE.sendClick(%f, %f) end", x, y))
    end)

    -- Full pointer-event relay for the wsstream feed (move/down/up/wheel).
    -- Validated + forwarded to GE; the app synthesizes matching Win32 input.
    local WS_ACTIONS = { move = true, down = true, up = true, wheel = true }
    local WS_BUTTONS = { left = true, right = true, middle = true }
    M.registerLuaCallback("wsInput", function(args)
        if not args or type(args.action) ~= "string" then return end
        if not WS_ACTIONS[args.action] then return end
        local button = tostring(args.button or "left")
        if not WS_BUTTONS[button] then button = "left" end
        local x = math.max(0, math.min(1, tonumber(args.x) or 0))
        local y = math.max(0, math.min(1, tonumber(args.y) or 0))
        local delta = math.floor(tonumber(args.delta) or 0)
        local held = math.floor(tonumber(args.held) or 0)
        obj:queueGameEngineLua(string.format("if mediaBridgeGE and mediaBridgeGE.sendInput then mediaBridgeGE.sendInput(%q, %f, %f, %q, %d, %d) end", tostring(args.action), x, y, button, delta, held))
    end)

    -- Register with screenService immediately so clicks work
    -- regardless of whether a player is seated
    obj:queueGameEngineLua(serviceExtension .. ".setFocusCar(" .. objectId .. ")")

    -- Register callback for HTML ready signal
    M.registerLuaCallback("bbcScreenReady", onHTMLReady)

    -- TEMPORARY DEBUG: channel for JS-side probes
    M.registerLuaCallback("bbcScreenDebug", function(args)
        dlog('I', "screenInput", "JS debug: " .. tostring(args and args.msg))
    end)
end

M.reset = reset
M.init = init
M.initLastStage = initLastStage
M.addScreen = addScreen
M.onScreenReady = onScreenReady
M.queueIgnitionEvent = queueIgnitionEvent
M.updateGFX = updateGFX

-- Coordinate-based input events (mouse clicks, drags, etc.)
local function inputCoordinate(eventData)
    if screenLocked then return end
    callJS("screenInput.onInput", eventData)
end

M.inputCoordinate = inputCoordinate

-- Hover state changes (mouse enter/leave trigger boxes)
local function onHover(boxId)
    callJS("screenInput.onHover", boxId)
end

M.onHover = onHover

-- Trigger events (press, click, hold, drag on trigger volumes)
local function onTrigger(eventData)
    callJS("screenInput.onTrigger", eventData)
end

M.onTrigger = onTrigger

M.pushToScreens = function(fnName, data)
    callJS(fnName, data)
end

-- Engine-paced push via htmlTexture streamJS: the engine coalesces per stream
-- name and delivers at the webview's texture fps, so a burst of queued
-- commands can't stampede callJS with stale frames. Same signature as
-- pushToScreens; falls back to it per-screen if streamJS is unavailable.
M.pushToScreensStream = function(fnName, data)
    if not htmlReady then return end
    for i = 1, #gaugeHTMLTextures do
        local t = gaugeHTMLTextures[i]
        if t.streamJS then
            t:streamJS(fnName, fnName, data)
        else
            t:callJS(fnName, data)
        end
    end
end

-- Exported so other vehicle controllers (e.g. lightingController) can push to the screen.
-- Previously callJS was only a local, so screenInput.callJS was nil for everyone else.
M.callJS = callJS

-- Queried from GE side (mediaBridgeGE) before pushing media updates
M.isHtmlReady = function()
    return htmlReady
end

-- Data load callbacks
local function onPersistLoaded(callbackId, packedData)
    local data = lpack.decode(packedData)
    callJS("persistCallback", {
        type = "loaded",
        callbackId = callbackId,
        data = data
    })
end

M.onPersistLoaded = onPersistLoaded

-- Data merged callbacks
local function onPersistMerged(callbackId, packedData, packedSources)
    local data = lpack.decode(packedData)
    local sources = lpack.decode(packedSources)
    callJS("persistCallback", {
        type = "merged",
        callbackId = callbackId,
        data = data,
        sources = sources
    })
end

M.onPersistMerged = onPersistMerged

-- Generic data callback handler
local function onPersistCallback(dataType, callbackId, packedData)
    local data = lpack.decode(packedData)
    callJS("persistCallback", {
        type = dataType,
        callbackId = callbackId,
        data = data
    })
end

M.onPersistCallback = onPersistCallback

-- Simple sound test function - calls testSound controller if available
local function playTestSound()
    if controller.testSound and controller.testSound.playClick then
        controller.testSound.playClick()
    end
end

M.playTestSound = playTestSound

local function registerLuaCallback(functionName, handler)
    luaCallbackHandlers[functionName] = handler
    dlog('D', 'screenInput', 'Registered Lua callback: ' .. tostring(functionName))
    -- Debug: print all registered handlers
    local handlers = ""
    for k, _ in pairs(luaCallbackHandlers) do
        handlers = handlers .. k .. ", "
    end
    dlog('D', 'screenInput', 'Current handlers: ' .. handlers)
end

local function onLuaCallback(functionName, args)
    dlog('D', 'screenInput.onLuaCallback', 'Received callback: ' .. tostring(functionName) .. ', handler exists: ' .. tostring(luaCallbackHandlers[functionName] ~= nil))
    if functionName == "bbcScreenCmd" and args and args.cmd then
        -- mediaBridgeGE lives in GE Lua; forward the command there
        local cmd = tostring(args.cmd):gsub("\\", "\\\\"):gsub('"', '\\"')
        dlog('I', "screenInput", "Screen command received: " .. cmd)
        obj:queueGameEngineLua('if mediaBridgeGE then mediaBridgeGE.handleScreenCmd("' .. cmd .. '", {}) else log("W", "screenInput", "mediaBridgeGE not loaded!") end')
        return
    end

    if luaCallbackHandlers[functionName] then
        dlog('D', 'screenInput.onLuaCallback', 'Calling handler for: ' .. tostring(functionName))
        luaCallbackHandlers[functionName](args)
    else
        dlog("W", "screenInput.onLuaCallback", "No handler registered for: " .. tostring(functionName))
        -- Debug: print all registered handlers
        local handlers = ""
        for k, _ in pairs(luaCallbackHandlers) do
            handlers = handlers .. k .. ", "
        end
        dlog("W", "screenInput.onLuaCallback", "Registered handlers: " .. handlers)
    end
end

-- Handle callVehicleLua from JavaScript via screenService
local function onCallVehicleLua(functionName, args)
    dlog('D', 'screenInput.onCallVehicleLua', 'Function: ' .. tostring(functionName) .. ', Args type: ' .. type(args))
    onLuaCallback(functionName, args)
end

M.registerLuaCallback = registerLuaCallback
M.onLuaCallback = onLuaCallback
M.onCallVehicleLua = onCallVehicleLua

rawset(_G, "screenInput", M)
return M