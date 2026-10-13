-- bbcWsFeed - GE websocket client for the JPEG stream feeds
-- Connects OUT to an external WS server (which sends BINARY JPEG FRAMES) and
-- relays the feed to the BBC universal screen.
--
-- Port order matters: 8081 = the audio_app Opera-box stream (game-sized
-- ~1280px frames that fit the Lua->CEF relay), 8080 = standalone
-- window_streamer fallback (full-window frames can exceed relay limits).
-- If nothing answers on the current port within CONNECT_TIMEOUT, the client
-- rotates to the next one automatically.
--
-- Why via lua: the screen's CEF webview cannot open the socket directly,
-- so GE receives each JPEG frame, base64-encodes it (mime.b64) and pushes
-- it to the vehicle screen as a data-URL string. The wsstream app renders
-- window.updateWSFeed payloads containing {img = <base64 jpeg>}.
--
-- Usage: extensions.load('bbcWsFeed')  (also auto-loaded by modscript.lua)
--   bbcWsFeed.getStatus()  bbcWsFeed.getLastText()  bbcWsFeed.send("hello")

local M = {}

local WS_HOST = "127.0.0.1"
local WS_PORTS = { 8081, 8080 }
local WS_PATH = "/"
local WS_PROTOCOL = "" -- empty = accept any protocol the server speaks
local portIdx = 1
local WS_PORT = WS_PORTS[portIdx]
local WS_URL = "ws://" .. WS_HOST .. ":" .. WS_PORT
local CONNECT_TIMEOUT = 8.0 -- no 'C' event within this long: try next port
local attemptStart = os.clock()

-- Video relay tuning: JPEG frames are base64'd (+33%) and ferried through
-- GE->vehicle queueLuaCommand + callJS as one big string per push. The relay
-- caps itself at IMG_MAX_FPS (~30) — vehicle Lua flushes queued commands once
-- per tick, so faster arrival only causes bursty display, not more smoothness.
-- Raise via bbcWsFeed.setMaxFps(n) only on the Direct socket path.
local IMG_MAX_FPS = 30
local IMG_MAX_BYTES = 500 * 1024

local client = nil
local connected = false
local lastData = nil      -- text payload table or {binary=true,...} meta
local lastRaw = ""        -- text only (truncated); NEVER binary bytes
local lastText = ""       -- last string message from server (status/debug)
local lastMsgTime = 0
local lastPushTime = 0
local lastImgPushTime = 0
local pendingImg = nil    -- base64 jpeg waiting for next screen push
local reconnectTimer = 0
local RECONNECT_INTERVAL = 5.0
local PUSH_INTERVAL = 0 -- push every arriving frame (no time gate); raise to e.g. 0.033 to cap at ~30 Hz
local frameCount = 0
local lastFrameBytes = 0
local lastFrameTime = 0
local droppedFrames = 0

local mimeMod = mime
if not mimeMod then
  local ok, m = pcall(require, "mime")
  if ok then mimeMod = m end
end

local function dlog(level, msg)
  log(level, "bbcWsFeed", tostring(msg))
end

local function decodePayload(raw)
  if type(raw) ~= "string" or raw == "" then return nil end
  -- binary JPEG frame? (SOI marker FFD8, or NUL byte, or just huge)
  if #raw > 4096 or raw:byte(1) == 0xFF and raw:byte(2) == 0xD8 or raw:find("\0", 1, true) then
    return nil -- handled as binary by caller
  end
  local ok, decoded = pcall(jsonDecode, raw)
  if ok and decoded ~= nil then
    return decoded
  end
  return { raw = raw }
end

local function isBinaryMsg(msg)
  if type(msg) ~= "string" then return false end
  if #msg > 4096 then return true end
  if #msg >= 2 and msg:byte(1) == 0xFF and msg:byte(2) == 0xD8 then return true end
  if msg:find("\0", 1, true) then return true end
  return false
end

local function setPort(i)
  portIdx = ((i - 1) % #WS_PORTS) + 1
  WS_PORT = WS_PORTS[portIdx]
  WS_URL = "ws://" .. WS_HOST .. ":" .. WS_PORT
end

local disconnect -- forward declaration (rotatePort below runs before its definition)

local function connect()
  if client then return true end
  if not BNGWSClient or not BNGWSClient.getOrCreate then
    dlog("W", "BNGWSClient not available in this build, cannot connect to " .. WS_URL)
    return false
  end
  local ok, c = pcall(BNGWSClient.getOrCreate, WS_HOST, WS_PORT, WS_PATH, WS_PROTOCOL)
  if not ok or not c then
    dlog("W", "getOrCreate failed for " .. WS_URL .. " (" .. tostring(c) .. ")")
    return false
  end
  client = c
  attemptStart = os.clock()
  dlog("I", "connecting to " .. WS_URL)
  return true
end

-- Give up on a silent port (refused / dead server: no 'C' event arrives) and
-- move to the next one. Called from onUpdate, so rotation is automatic.
local function rotatePort(reason)
  dlog("W", "no connection on " .. WS_URL .. " (" .. tostring(reason) .. "), trying next port")
  disconnect()
  setPort(portIdx + 1)
  connect()
end

disconnect = function()
  if client and BNGWSClient and BNGWSClient.destroy then
    pcall(BNGWSClient.destroy, client)
  end
  client = nil
  pendingImg = nil
  if connected then
    connected = false
    guihooks.trigger("BBCWsFeed", { connected = false, url = WS_URL })
  end
end

-- Escape a string for embedding inside a double quoted Lua string literal
local function escapeLuaString(s)
  return (s:gsub("\\", "\\\\"):gsub('"', '\\"'))
end

-- Plan B fast path: drop the raw binary JPEG to a file inside the mod. The
-- screen page polls it via local:// (the VFS reads unpacked dirs from disk),
-- so frames reach CEF with NO base64, NO queueLuaCommand and NO JSON.
local LIVE_DIR = "mods/unpacked/bbc_stereo_0.6/vehicles/common/BBC_Universal_Screen/live/"
local livePath = nil
local function writeLiveFrame(msg)
  if not livePath then
    local ok, up = pcall(function() return FS and FS:getUserPath() end)
    if not ok or not up then return false end
    livePath = (up:gsub("\\", "/")) .. LIVE_DIR
    pcall(function() FS:directoryCreate(livePath, true) end)
  end
  -- temp+rename so the page never reads a half-written JPEG
  local tmp = livePath .. "frame.jpg.tmp"
  local f = io.open(tmp, "wb")
  if not f then return false end
  f:write(msg)
  f:close()
  os.remove(livePath .. "frame.jpg")
  return (os.rename(tmp, livePath .. "frame.jpg") ~= nil)
end

-- Relay latest feed to the vehicle's universal screen.
-- screenInput.pushToScreens exists in vehicle lua and forwards to every
-- htmlTexture screen as callJS("updateWSFeed", data), which the wsstream
-- app implements. Same lpack pattern screenService/mediaBridgeGE use.
-- pendingImg is attached once then cleared, so the 1 Hz heartbeat below
-- only resends lightweight stats until the next frame arrives.
--
-- Image frames skip lpack + escapeLuaString entirely: base64 output is only
-- [A-Za-z0-9+/=], which is literal-safe in a Lua string. The current lpack
-- path gsub-scans and rebuilds the ~130KB binary payload twice per frame and
-- makes the vehicle Lua re-parse it as a string literal — the direct push
-- removes that whole round trip (several ms of string churn at 30 Hz).
local function pushImgDirect()
  -- pendingImg must exist; caller checked. WS_URL contains no quotes/backslashes.
  -- string.format builds the command in ONE pass — a chained .. concat
  -- allocates ~14 intermediates (several near full ~150KB size) per frame.
  -- pushToScreensStream (engine streamJS channel) is preferred: the engine
  -- coalesces per stream name and paces delivery at the webview's fps, so
  -- bursts in the vehicle command queue can't stall the view. Falls back to
  -- pushToScreens (plain callJS) if the vehicle Lua predates it.
  local cmd = string.format([[if screenInput and screenInput.isHtmlReady and screenInput.isHtmlReady() then
    local push = screenInput.pushToScreensStream or screenInput.pushToScreens
    push("updateWSFeed", {connected=%s, url="%s", frames=%d, bytes=%d, dropped=%d, img="%s"})
  end]],
    tostring(connected), WS_URL, frameCount, lastFrameBytes, droppedFrames, pendingImg)
  local veh = be and be.getPlayerVehicle and be:getPlayerVehicle(0) or nil
  if not veh then return false end
  veh:queueLuaCommand(cmd)
  return true
end

local function pushToScreen(force)
  local now = os.clock()
  if not force and (now - lastPushTime) < PUSH_INTERVAL then return end
  lastPushTime = now
  local veh = be and be.getPlayerVehicle and be:getPlayerVehicle(0) or nil
  if not veh then return end
  if pendingImg then
    -- New image frame due: fast path, no lpack round trip.
    if pushImgDirect() then
      pendingImg = nil
      return
    end
    -- veh vanished between the checks: fall through to the slow path so
    -- the frame still rides the next heartbeat instead of being lost.
  end
  local payload = {
    connected = connected,
    url = WS_URL,
    data = lastData,
    raw = lastRaw, -- text only, truncated; "" when last item was a binary frame
    isBinary = lastData and lastData.binary == true or nil,
    bytes = lastFrameBytes,
    frames = frameCount,
    dropped = droppedFrames,
    text = lastText,
    img = pendingImg, -- base64 jpeg (nil unless a new frame is due)
  }
  pendingImg = nil
  local ok, packed = pcall(lpack.encode, payload)
  if not ok or not packed then return end
  veh:queueLuaCommand([[if screenInput and screenInput.isHtmlReady and screenInput.isHtmlReady() then
    screenInput.pushToScreens("updateWSFeed", lpack.decode("]] .. escapeLuaString(packed) .. [["))
  end]])
end

local function handleBinaryFrame(msg)
  frameCount = frameCount + 1
  lastFrameBytes = #msg
  lastFrameTime = os.clock()
  lastMsgTime = lastFrameTime
  lastData = { binary = true, bytes = #msg, frames = frameCount }
  lastRaw = ""
  if #msg > IMG_MAX_BYTES then
    droppedFrames = droppedFrames + 1
    dlog("W", "frame " .. frameCount .. " too big (" .. #msg .. " bytes), dropped")
  elseif not mimeMod or not mimeMod.b64 then
    droppedFrames = droppedFrames + 1
    dlog("W", "mime.b64 unavailable, cannot relay frame " .. frameCount)
  else
    local ok, b64 = pcall(mimeMod.b64, msg)
    if ok and type(b64) == "string" and b64 ~= "" then
      -- Strip line wrapping (mime.b64 can break at 76 cols): the direct push
      -- embeds this in a Lua string literal, which allows no literal newlines.
      b64 = (b64:gsub("%s", ""))
      -- Hard rate gate (arrival-time based, jitter tolerant): frames arriving
      -- faster than ~IMG_MAX_FPS are NOT staged at all. Staging-everything
      -- made the gate count-only, so a 60fps stream still pushed 60 heavy
      -- commands/s in vehicle-tick bursts. 0.9/IMG_MAX_FPS lets a true 30fps
      -- stream pass through gaps as small as ~27ms unharmed.
      if (lastFrameTime - lastImgPushTime) >= (0.9 / IMG_MAX_FPS) then
        lastImgPushTime = lastFrameTime
        pendingImg = b64
        writeLiveFrame(msg) -- Plan B: raw binary drop for the local:// poller
      else
        droppedFrames = droppedFrames + 1
      end
    else
      droppedFrames = droppedFrames + 1
      dlog("W", "base64 encode failed for frame " .. frameCount)
    end
  end
  pushToScreen(false)
end

local function handleEvent(evt)
  if type(evt) ~= "table" then return end
  local t = evt.type
  if t == "C" then
    if not connected then
      connected = true
      dlog("I", "connected to " .. WS_URL)
      guihooks.trigger("BBCWsFeed", { connected = true, url = WS_URL })
      pushToScreen(true)
    end
  elseif t == "DC" then
    if connected then
      connected = false
      dlog("W", "disconnected from " .. WS_URL)
      guihooks.trigger("BBCWsFeed", { connected = false, url = WS_URL })
      pushToScreen(true)
    end
    -- grace window on this port for a recovering server; rotation (below)
    -- moves on if nothing answers in time
    attemptStart = os.clock()
  elseif t == "D" then
    local msg = evt.msg
    if msg == nil or msg == "" then return end
    if type(msg) ~= "string" then msg = tostring(msg) end
    -- treat first data as proof of connection even if no explicit 'C' arrived
    if not connected then
      connected = true
      guihooks.trigger("BBCWsFeed", { connected = true, url = WS_URL })
    end
    if isBinaryMsg(msg) then
      handleBinaryFrame(msg)
      guihooks.trigger("BBCWsFeedData", { url = WS_URL, binary = true, bytes = #msg, frames = frameCount })
    else
      lastRaw = msg:sub(1, 512)
      lastText = lastRaw
      lastData = decodePayload(msg)
      lastMsgTime = os.clock()
      guihooks.trigger("BBCWsFeedData", { url = WS_URL, data = lastData, raw = lastRaw, frames = frameCount, bytes = lastFrameBytes })
      pushToScreen(false)
    end
  end
end

local function onUpdate(dt)
  if not client then
    reconnectTimer = reconnectTimer + (dt or 0)
    if reconnectTimer >= RECONNECT_INTERVAL then
      reconnectTimer = 0
      connect()
    end
    return
  end

  local ok, err = pcall(client.update, client)
  if not ok then
    dlog("W", "client:update failed (" .. tostring(err) .. "), reconnecting")
    disconnect()
    return
  end

  local ok2, events = pcall(client.getPeerEvents, client)
  if ok2 and type(events) == "table" then
    for _, evt in ipairs(events) do
      handleEvent(evt)
    end
  end

  -- Connected nowhere and the current port stays silent (nothing listening):
  -- rotate so the game finds whichever streamer is actually running.
  if client and not connected and (os.clock() - attemptStart) > CONNECT_TIMEOUT then
    rotatePort("silent for " .. CONNECT_TIMEOUT .. "s")
    return
  end

  -- heartbeat so the screen status line stays fresh (1 Hz, stats only)
  if connected and (os.clock() - lastPushTime) > 1.0 then
    pushToScreen(true)
  end
end

local function onExtensionLoaded()
  dlog("I", "loaded, dialing " .. WS_URL .. " (extensions.load('bbcWsFeed'))")
  connect()
end

local function onExtensionUnloaded()
  disconnect()
end

M.connect = connect
M.disconnect = disconnect
M.reconnect = function() disconnect() reconnectTimer = RECONNECT_INTERVAL return connect() end
-- Manual override from the GE console, e.g. bbcWsFeed.usePort(8080)
M.usePort = function(port)
  for i, p in ipairs(WS_PORTS) do
    if p == tonumber(port) then
      disconnect()
      setPort(i)
      reconnectTimer = RECONNECT_INTERVAL
      return connect()
    end
  end
  return false, "known ports: " .. table.concat(WS_PORTS, ", ")
end
M.send = function(msg)
  if not client then return false end
  local ok, err = pcall(client.sendData, client, tostring(msg))
  if not ok then dlog("W", "send failed: " .. tostring(err)) return false end
  return true
end
M.setMaxFps = function(fps) IMG_MAX_FPS = math.max(1, math.min(120, tonumber(fps) or 60)) return IMG_MAX_FPS end
M.getStatus = function()
  return { connected = connected, url = WS_URL, host = WS_HOST, port = WS_PORT, lastMsgAge = lastMsgTime > 0 and (os.clock() - lastMsgTime) or -1, frames = frameCount, bytes = lastFrameBytes, dropped = droppedFrames, text = lastText }
end
M.getLastData = function() return lastData end
M.getLastRaw = function() return lastRaw end
M.getLastText = function() return lastText end
M.getFrameStats = function() return { frames = frameCount, bytes = lastFrameBytes, dropped = droppedFrames } end

M.onUpdate = onUpdate
M.onExtensionLoaded = onExtensionLoaded
M.onExtensionUnloaded = onExtensionUnloaded

rawset(_G, "bbcWsFeed", M)
return M
