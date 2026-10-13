-- subwooferWsGE - GE websocket bridge for the subwoofer controller (WS edition)
--
-- Connects OUT to the external BBC Stereo app over WEBSOCKET (instead of the
-- HTTP polling done by the original subwooferController) and relays each JSON
-- payload into the player vehicle, where subwooferControllerWs.lua consumes it.
--
-- Vehicle Lua has no websocket bindings (BNGWSClient is GE-only), so this
-- extension owns the socket and the vehicle controller only reads the pushed
-- values. Same event API as bbcWsFeed: BNGWSClient.getOrCreate(host, port,
-- path, protocol) -> client:update() pump -> client:getPeerEvents() with
-- event types 'C' (connected), 'DC' (disconnected), 'D' (data).
--
-- Expected payload (same schema the HTTP /values endpoint serves):
--   {sub=, bass=, mid=, treble=, title=, artist=, album=,
--    current_time=, total_time=, is_playing=, thumbnail=}
--
-- Port rotation: if nothing answers on the current port within
-- CONNECT_TIMEOUT, the client rotates to the next one automatically, exactly
-- like bbcWsFeed. Default target is the app's websocket port 5001
-- (ws://127.0.0.1:5001/); add ports to WS_PORTS to rotate, or pin one with
-- subwooferWsGE.setTarget().
--
-- Usage: extensions.load('subwooferWsGE')  (auto-loaded by modscript.lua)
--   subwooferWsGE.getStatus()
--   subwooferWsGE.setTarget(host, port, path)   -- pin one endpoint
--   subwooferWsGE.send("hello")                 -- text back to the app

local M = {}

local WS_HOST = "127.0.0.1"
local WS_PORTS = { 5001 }
local WS_PATH = "/"
local WS_PROTOCOL = "" -- empty = accept any protocol the server speaks

local portIdx = 1
local WS_PORT = WS_PORTS[portIdx]
local WS_URL = "ws://" .. WS_HOST .. ":" .. WS_PORT

local CONNECT_TIMEOUT = 8.0 -- no 'C' event within this long: try next port
local RECONNECT_INTERVAL = 5.0
local PUSH_MIN_INTERVAL = 1 / 120 -- cap relays into the vehicle at 120 Hz

local client = nil
local connected = false
local attemptStart = os.clock()
local reconnectTimer = 0
local lastPushTime = 0

-- stats
local msgCount = 0
local lastMsgTime = 0
local lastMsgAge = -1
local lastValidPayload = nil
local droppedCount = 0

local function dlog(level, msg)
  log(level, "subwooferWsGE", tostring(msg))
end

-- Escape a JSON text for embedding inside a double-quoted Lua string literal.
-- %q is not used on purpose: its output can contain "]]", which would
-- terminate a long-bracket literal if one is ever used around it, and it
-- emits line continuations for newlines. Plain escaping keeps the command a
-- single line.
local function escapeLuaString(s)
  return (s:gsub("[\\%z\1-\31\"]", function(c)
    if c == "\\" then return "\\\\" end
    if c == "\"" then return "\\\"" end
    if c == "\n" then return "\\n" end
    if c == "\r" then return "\\r" end
    return string.format("\\%03d", c:byte())
  end))
end

local function setPort(i)
  portIdx = ((i - 1) % #WS_PORTS) + 1
  WS_PORT = WS_PORTS[portIdx]
  WS_URL = "ws://" .. WS_HOST .. ":" .. WS_PORT .. WS_PATH
end

local disconnect -- forward declaration (rotatePort runs before its definition)

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
  if connected then
    connected = false
    dlog("W", "disconnected from " .. WS_URL)
  end
end

-- Relay one JSON text message into the player vehicle. The vehicle controller
-- (subwooferControllerWs) defines the global subwooferWs_receive; the guard
-- makes the push a harmless no-op on vehicles without it.
local function pushToVehicle(raw)
  local now = os.clock()
  if (now - lastPushTime) < PUSH_MIN_INTERVAL then return end
  lastPushTime = now
  local veh = be and be.getPlayerVehicle and be:getPlayerVehicle(0) or nil
  if not veh then return end
  local cmd = "if subwooferWs_receive then subwooferWs_receive(\"" .. escapeLuaString(raw) .. "\") end"
  veh:queueLuaCommand(cmd)
end

-- Accept the payload only if it looks like subwoofer data: audio bands or
-- media info, either flat or nested under "data".
local function handleDataMessage(raw)
  local ok, data = pcall(jsonDecode, raw)
  if not ok or type(data) ~= "table" then
    droppedCount = droppedCount + 1
    return
  end
  if type(data.data) == "table" then data = data.data end

  local hasAudio = data.sub ~= nil or data.bass ~= nil or data.mid ~= nil or data.treble ~= nil
  local hasMedia = data.title ~= nil or data.artist ~= nil or data.album ~= nil
    or data.current_time ~= nil or data.total_time ~= nil or data.is_playing ~= nil
  if not hasAudio and not hasMedia then
    droppedCount = droppedCount + 1
    return
  end

  if not connected then
    connected = true
    dlog("I", "connected to " .. WS_URL .. " (data before 'C' event)")
  end
  msgCount = msgCount + 1
  lastMsgTime = os.clock()
  lastValidPayload = data
  pushToVehicle(raw)
end

local function handleEvent(evt)
  if type(evt) ~= "table" then return end
  local t = evt.type
  if t == "C" then
    if not connected then
      connected = true
      dlog("I", "connected to " .. WS_URL)
    end
  elseif t == "DC" then
    if connected then
      connected = false
      dlog("W", "disconnected from " .. WS_URL)
    end
    -- grace window on this port for a recovering server; rotation (below)
    -- moves on if nothing answers in time
    attemptStart = os.clock()
  elseif t == "D" then
    local msg = evt.msg
    if msg == nil or msg == "" then return end
    if type(msg) ~= "string" then msg = tostring(msg) end
    -- binary frames (e.g. a video feed sharing the endpoint) are not ours.
    -- NOTE: size alone must NOT decide this - valid JSON payloads with a
    -- base64 thumbnail exceed 4KB. JSON text never contains NUL bytes or a
    -- JPEG SOI header, so those two are the only binary signals.
    if (#msg >= 2 and msg:byte(1) == 0xFF and msg:byte(2) == 0xD8)
      or msg:find("\0", 1, true) then
      droppedCount = droppedCount + 1
      return
    end
    handleDataMessage(msg)
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
  -- rotate so the bridge finds whichever endpoint the app actually serves.
  if client and not connected and (os.clock() - attemptStart) > CONNECT_TIMEOUT then
    rotatePort("silent for " .. CONNECT_TIMEOUT .. "s")
    return
  end
end

local function onExtensionLoaded()
  dlog("I", "loaded, dialing " .. WS_URL .. " (extensions.load('subwooferWsGE'))")
  connect()
end

local function onExtensionUnloaded()
  disconnect()
end

M.connect = connect
M.disconnect = disconnect
M.reconnect = function() disconnect() reconnectTimer = RECONNECT_INTERVAL return connect() end

-- Manual override from the GE console, e.g. subwooferWsGE.setTarget("127.0.0.1", 5000, "/ws")
M.setTarget = function(host, port, path)
  WS_HOST = tostring(host or WS_HOST)
  WS_PATH = tostring(path or "/")
  local found = false
  for i, p in ipairs(WS_PORTS) do
    if p == tonumber(port) then setPort(i); found = true end
  end
  if not found then
    WS_PORTS = { tonumber(port) or WS_PORT }
    setPort(1)
  end
  WS_URL = "ws://" .. WS_HOST .. ":" .. WS_PORT .. WS_PATH
  reconnectTimer = RECONNECT_INTERVAL
  return connect()
end

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

M.getStatus = function()
  lastMsgAge = lastMsgTime > 0 and (os.clock() - lastMsgTime) or -1
  return {
    connected = connected,
    url = WS_URL,
    host = WS_HOST,
    port = WS_PORT,
    path = WS_PATH,
    lastMsgAge = lastMsgAge,
    messages = msgCount,
    dropped = droppedCount,
    hasPayload = lastValidPayload ~= nil,
  }
end
M.getLastPayload = function() return lastValidPayload end

M.onUpdate = onUpdate
M.onExtensionLoaded = onExtensionLoaded
M.onExtensionUnloaded = onExtensionUnloaded

rawset(_G, "subwooferWsGE", M)
return M
