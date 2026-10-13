-- mediaBridgeGE - Bridge media info from electrics to screen
-- Provides media info (title, artist, album, time, thumbnail) to screen webview

local M = {}
M.dependencies = {"ui_imgui"}

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "mediaBridgeGE", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

local cachedMedia = {
  title = "",
  artist = "",
  album = "",
  currentTime = 0,
  totalTime = 0,
  isPlaying = false,
  thumbnail = ""
}

local cachedVolume = 50  -- persistent volume value (0-100)

-- EQ Bands (15 bands, -16 to +16 dB)
local cachedEq = {}
for i = 1, 15 do cachedEq[i] = 0 end

local lastMediaUpdate = 0
local MEDIA_UPDATE_INTERVAL = 0.2 -- 200ms (5Hz)

-- UDP command channel to the external BBC Stereo app (port 5001),
-- same channel the mediaInfoGE imgui window uses for its buttons
local hasSocket = false
local udp = nil
local json = nil
do
  local status, socketModule = pcall(require, "socket")
  if status then
    hasSocket = true
    udp = socketModule.udp()
    udp:settimeout(0)
    udp:setpeername("127.0.0.1", 5001)
    json = dofile("lua/vehicle/controller/dkjson.lua")
    dlog('I', "mediaBridgeGE", "Socket UDP module available - screen media controls enabled on port 5001")
  else
    dlog('W', "mediaBridgeGE", "Socket module not available - screen media controls disabled")
  end
end

local function sendMediaCommand(command, extra)
  if not hasSocket or not udp then
    dlog('W', "mediaBridgeGE", "Cannot send command - UDP module not available")
    return
  end
  local payloadTable = {command = command}
  if extra then
    for k, v in pairs(extra) do payloadTable[k] = v end
  end
  local payload = json.encode(payloadTable)
  local success, err = udp:send(payload)
  if success then
    dlog('I', "mediaBridgeGE", "Sent media command via UDP: " .. payload)
  else
    dlog('W', "mediaBridgeGE", "Failed to send media command: " .. tostring(err))
  end
end

-- Click relay for the wsstream screen feed: normalized 0..1 coords travel
-- over the already-open UDP 5001 channel; the app maps them into its
-- embedded Opera window and synthesizes the click there.
local function sendClick(x, y)
  sendMediaCommand("ws_click", { x = tonumber(x) or 0, y = tonumber(y) or 0 })
end

-- Screen-typing key relay -------------------------------------------------
-- Physical keys arrive as mod input ACTIONS (bbc_screen_keys.json), NOT from
-- CEF: the engine consumes bound keys before any htmlTexture sees them, so an
-- in-car page can never observe typing directly (this is also why Space never
-- reached the flappy game). Modifier state is tracked here so shifted chars
-- and symbols resolve correctly; everything is hover-gated, and hover-leave
-- flushes key-ups via releaseAllKeys so nothing sticks down in the browser.
local keyMods = { shift = false, ctrl = false, alt = false }
local MOD_CODES = { ShiftLeft = "shift", ShiftRight = "shift", ControlLeft = "ctrl", ControlRight = "ctrl", AltLeft = "alt" }
local SHIFTED_DIGITS = { ["1"]="!", ["2"]="@", ["3"]="#", ["4"]="$", ["5"]="%", ["6"]="^", ["7"]="&", ["8"]="*", ["9"]="(", ["0"]=")" }
local SHIFTED_PUNCT = { [";"]=":", ["="]="+", [","]="<", ["-"]="_", ["."]=">", ["/"]="?", ["`"]="~", ["["]="{", ["\\"]="|", ["]"]="}", ["'"]='"' }
local function resolveKeyChar(code)
  local letter = code:match("^Key([A-Z])$")
  if letter then
    return keyMods.shift and letter or letter:lower()
  end
  local digit = code:match("^Digit([0-9])$")
  if digit then
    return (keyMods.shift and SHIFTED_DIGITS[digit]) or digit
  end
  local punct = {
    Semicolon=";", Equal="=", Comma=",", Minus="-", Period=".", Slash="/",
    Backquote="`", BracketLeft="[", Backslash="\\", BracketRight="]", Quote="'",
  }
  local p = punct[code]
  if p then
    return (keyMods.shift and SHIFTED_PUNCT[p]) or p
  end
  if code == "Space" then return " " end
  if code == "Enter" then return "\r" end
  if code == "Tab" then return "\t" end
  if code == "Backspace" then return "\b" end
  return nil -- arrows, delete, modifiers...: virtual-key only, no text
end

local heldKeyCodes = {}
local function sendKey(code, down)
  code = tostring(code or "")
  if code == "" then return end
  if not (screenService and screenService.isScreenHovered and screenService.isScreenHovered()) then return end
  down = (down == true)
  if MOD_CODES[code] then keyMods[MOD_CODES[code]] = down end
  if down then heldKeyCodes[code] = true else heldKeyCodes[code] = nil end
  local ch = resolveKeyChar(code)
  sendMediaCommand("ws_input", { action = "key", code = code, key = ch, down = down and 1 or 0 })
end

-- Push key-ups for everything held (hover-leave / vehicle reset safety).
local function releaseAllKeys()
  for code, _ in pairs(heldKeyCodes) do
    sendMediaCommand("ws_input", { action = "key", code = code, down = 0 })
  end
  heldKeyCodes = {}
  keyMods = { shift = false, ctrl = false, alt = false }
end

-- Full pointer-event relay for the wsstream feed. Same UDP 5001 channel;
-- the app maps the normalized coords into its embedded Opera window.
local function sendInput(action, x, y, button, delta, held)
  -- DIAG-WHEEL: proves the wheel made it to the UDP relay (beamng.log)
  if action == "wheel" then
    log('I', "mediaBridgeGE", string.format("wheel relay x=%.3f y=%.3f delta=%s held=%s",
      tonumber(x) or 0, tonumber(y) or 0, tostring(delta), tostring(held)))
  end
  sendMediaCommand("ws_input", {
    action = tostring(action or "move"),
    x = tonumber(x) or 0,
    y = tonumber(y) or 0,
    button = tostring(button or "left"),
    delta = math.floor(tonumber(delta) or 0),
    held = math.floor(tonumber(held) or 0),
  })
end

-- Screen update callback
local screenUpdateCallback = nil

local function updateMediaCache()
  local veh = be:getPlayerVehicle(0)
  if not veh then return end

  local vehId = veh:getId()
  local keys = {
    "mediaTitle", "mediaArtist", "mediaAlbum",
    "mediaCurrentTime", "mediaTotalTime", "mediaIsPlaying", "mediaThumbnail"
  }

  local changed = false
  for _, key in ipairs(keys) do
    local val = core_vehicleBridge.getCachedVehicleData(vehId, key)
    if val ~= nil then
      local field = key:gsub("^media", "")
      field = field:sub(1,1):lower() .. field:sub(2)
      if cachedMedia[field] ~= val then
        cachedMedia[field] = val
        changed = true
      end
    end
  end

  -- Notify screen if data changed
  if changed and screenUpdateCallback then
    screenUpdateCallback(cachedMedia)
  end
end

-- Escape a string for embedding inside a double quoted Lua string literal
local function escapeLuaString(s)
  return (s:gsub("\\", "\\\\"):gsub('"', '\\"'))
end

-- Push cached media info to the vehicle's HTML screen.
-- This runs in GE context; the vehicle screenInput lives in vehicle Lua,
-- so reach it with a queued command (same pattern screenService uses).
local function pushToScreen()
  local veh = be:getPlayerVehicle(0)
  if not veh then return end
  local packed = escapeLuaString(lpack.encode(cachedMedia))
  veh:queueLuaCommand([[if screenInput and screenInput.isHtmlReady and screenInput.isHtmlReady() then
    screenInput.pushToScreens("updateData", lpack.decode("]] .. packed .. [["))
  end]])
end

-- Push volume to vehicle electrics (called every frame to persist)
local function pushVolumeToVehicle()
  local veh = be:getPlayerVehicle(0)
  if not veh then return end
  -- Map 0-100 slider to -16 to +16 dB (app clamps to -16..+16)
  local db = -16 + (cachedVolume / 100) * 32
  veh:queueLuaCommand('electrics.values.volume = ' .. db)
end

-- Push EQ bands to vehicle electrics (called every frame to persist)
local function pushEqToVehicle()
  local veh = be:getPlayerVehicle(0)
  if not veh then return end
  for i = 1, 15 do
    veh:queueLuaCommand('electrics.values.eqband' .. i .. ' = ' .. cachedEq[i])
  end
end

local function onUpdate(dt)
  lastMediaUpdate = lastMediaUpdate + dt
  if lastMediaUpdate < MEDIA_UPDATE_INTERVAL then return end
  lastMediaUpdate = 0

  updateMediaCache()
  pushToScreen()
  pushVolumeToVehicle()  -- persist volume every frame
  pushEqToVehicle()      -- persist EQ every frame
end

M.onUpdate = onUpdate

-- Register callback for screen updates
M.registerScreenCallback = function(cb)
  screenUpdateCallback = cb
end

-- Handle screen commands (play/pause, next, previous, seek, volume, eq, lighting)
M.handleScreenCmd = function(cmd, args)
  dlog('I', "mediaBridgeGE", "handleScreenCmd: cmd=" .. tostring(cmd) .. " args=" .. tostring(args))
  local pct = args and args.pct or nil
  -- The screen sends seek as "seek:<fraction>" embedded in the cmd string
  if type(cmd) == "string" and cmd:sub(1, 5) == "seek:" then
    pct = tonumber(cmd:sub(6))
    cmd = "seek"
  end
  if cmd == "play_pause" then
    sendMediaCommand("play_pause")
    guihooks.trigger('MediaPlayPause', {})
  elseif cmd == "next" then
    sendMediaCommand("next")
    guihooks.trigger('MediaNext', {})
  elseif cmd == "previous" then
    sendMediaCommand("previous")
    guihooks.trigger('MediaPrevious', {})
  elseif cmd == "seek" and pct then
    sendMediaCommand("seek", {pct = pct})
    guihooks.trigger('MediaSeek', {pct = pct})
  elseif type(cmd) == "string" and cmd:sub(1, 7) == "volume:" then
    local vol = tonumber(cmd:sub(8))
    dlog('I', "mediaBridgeGE", "Volume command received: " .. tostring(vol))
    if vol then
      cachedVolume = vol
      -- Map 0-100 slider to -16 to +16 dB range (app clamps to -16..+16)
      local db = -16 + (vol / 100) * 32  -- 0% = -16dB, 100% = +16dB
      dlog('I', "mediaBridgeGE", "Mapped slider " .. vol .. "% to " .. db .. " dB")
      -- Immediately push to vehicle electrics (as dB)
      local veh = be:getPlayerVehicle(0)
      if veh then
        veh:queueLuaCommand('electrics.values.volume = ' .. db)
        dlog('I', "mediaBridgeGE", "Set electrics.values.volume = " .. db .. " dB")
      else
        dlog('W', "mediaBridgeGE", "No vehicle found for volume command")
      end
      sendMediaCommand("volume", {volume = db})
      guihooks.trigger('MediaVolume', {volume = db})
    end
  elseif type(cmd) == "string" and cmd:sub(1, 3) == "eq:" then
    -- Format: eq:band:value (e.g., "eq:1:3.5")
    local parts = {}
    for part in cmd:gmatch("[^:]+") do table.insert(parts, part) end
    if #parts == 3 then
      local band = tonumber(parts[2])
      local val = tonumber(parts[3])
      if band and band >= 1 and band <= 15 and val then
        -- Clamp to -16 to +16
        val = math.max(-16, math.min(16, val))
        cachedEq[band] = val
        dlog('I', "mediaBridgeGE", "EQ band " .. band .. " set to " .. val .. " dB")
        -- Immediately push to vehicle electrics
        local veh = be:getPlayerVehicle(0)
        if veh then
          veh:queueLuaCommand('electrics.values.eqband' .. band .. ' = ' .. val)
          dlog('I', "mediaBridgeGE", "Set electrics.values.eqband" .. band .. " = " .. val .. " dB")
        end
        sendMediaCommand("eq", {band = band, value = val})
        guihooks.trigger('MediaEq', {band = band, value = val})
      end
    end
  elseif type(cmd) == "string" and cmd:sub(1, 8) == "lighting:" then
    -- Forward lighting commands to vehicle lighting controller
    local payload = cmd:sub(9)  -- strip "lighting:"
    dlog('I', "mediaBridgeGE", "Lighting command received: " .. tostring(payload))
    local veh = be:getPlayerVehicle(0)
    if veh then
      veh:queueLuaCommand('lightingController.updateLighting(' .. payload .. ')')
      dlog('I', "mediaBridgeGE", "Forwarded lighting command to vehicle controller")
    else
      dlog('W', "mediaBridgeGE", "No vehicle found for lighting command")
    end
  end
end

-- Handle screen ready signal
M.onScreenReady = function()
  -- Could trigger an initial update here
end

M.onUpdate = onUpdate

M.sendClick = sendClick
M.sendKey = sendKey
M.releaseAllKeys = releaseAllKeys
M.sendInput = sendInput

-- Register callback for screen updates
M.registerScreenCallback = function(cb)
  screenUpdateCallback = cb
end

-- Getter for screen controllers
M.getMediaInfo = function()
  return cachedMedia
end

M.getVolume = function()
  -- Return dB value (-16 to +16)
  return -16 + (cachedVolume / 100) * 32
end

M.getEq = function()
  return cachedEq
end

M.getEqBand = function(band)
  return cachedEq[band] or 0
end

return M