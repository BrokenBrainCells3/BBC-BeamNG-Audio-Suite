-- Game Engine extension to display media info from the BBC Stereo app
-- Shows current song title, artist, album, time, and thumbnail

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
  dlog('I', "mediaInfoGE", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

local imgui = ui_imgui

-- UDP socket for sending media commands to the app (port 5001)
local hasSocket = false
local udp = nil
do
    local status, socketModule = pcall(require, "socket")
    if status then
        hasSocket = true
        udp = socketModule.udp()
        udp:settimeout(0)
        udp:setpeername("127.0.0.1", 5001)
        dlog('I', "mediaInfoGE", "Socket UDP module available - media controls enabled on port 5001")
    else
        dlog('W', "mediaInfoGE", "Socket module not available - media controls disabled")
    end
end

-- Pure Lua base64 decoder (no external dependencies)
local b64chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
local b64lookup = {}
for i = 1, #b64chars do
  b64lookup[b64chars:sub(i, i)] = i - 1
end

-- Pure Lua bit operations using math
local function lshift(x, n) return math.floor(x * 2^n) % 256 end
local function rshift(x, n) return math.floor(x / 2^n) end
local function band(x, y)
  local result = 0
  local bitval = 1
  while x > 0 or y > 0 do
    if x % 2 == 1 and y % 2 == 1 then result = result + bitval end
    x = math.floor(x / 2)
    y = math.floor(y / 2)
    bitval = bitval * 2
  end
  return result
end
local function bor(x, y)
  local result = 0
  local bitval = 1
  while x > 0 or y > 0 do
    if x % 2 == 1 or y % 2 == 1 then result = result + bitval end
    x = math.floor(x / 2)
    y = math.floor(y / 2)
    bitval = bitval * 2
  end
  return result
end

local function base64Decode(data)
  data = data:gsub('[^A-Za-z0-9+/=]', '')
  if #data % 4 ~= 0 then return nil end

  local result = {}
  for i = 1, #data, 4 do
    local c1 = b64lookup[data:sub(i, i)] or 0
    local c2 = b64lookup[data:sub(i+1, i+1)] or 0
    local c3 = b64lookup[data:sub(i+2, i+2)] or 0
    local c4 = b64lookup[data:sub(i+3, i+3)] or 0

    local b1 = bor(lshift(c1, 2), rshift(c2, 4))
    local b2 = bor(lshift(band(c2, 15), 4), rshift(c3, 2))
    local b3 = bor(lshift(band(c3, 3), 6), c4)

    result[#result + 1] = string.char(b1)
    if data:sub(i+2, i+2) ~= '=' then result[#result + 1] = string.char(b2) end
    if data:sub(i+3, i+3) ~= '=' then result[#result + 1] = string.char(b3) end
  end
  return table.concat(result)
end

local thumbnailTexture = nil
local cachedThumbnailBase64 = nil

local function writeFile(filename, data)
  local file, err = io.open(filename, 'w')
  if not file then
    dlog('W', "mediaInfoGE", "Error opening file for writing: " .. filename .. ": " .. tostring(err))
    return false
  end
  file:write(data)
  file:close()
  return true
end

local function loadThumbnailFromBase64(base64Data)
  if not base64Data or base64Data == "" then
    return nil
  end

  -- Strip data URI prefix if present
  local b64 = base64Data
  local prefixEnd = b64:find("base64,")
  if prefixEnd then
    b64 = b64:sub(prefixEnd + 7)
  end

  -- Decode base64
  local decoded = base64Decode(b64)
  if not decoded or #decoded == 0 then
    dlog('W', "mediaInfoGE", "Failed to decode base64 thumbnail")
    return nil
  end

  -- Save to temp file
  local tempPath = "settings/bbc_media_thumb_" .. os.time() .. ".png"
  local fullPath = tempPath -- writeFile uses relative to user folder
  writeFile(fullPath, decoded)

  -- Load texture
  local tex = imgui.ImTextureHandler(fullPath)
  if tex then
    cachedThumbnailBase64 = base64Data
    return tex
  end
  return nil
end

-- Window toggle
M.windowOpen = imgui.BoolPtr(false)

-- Cached media values
local cachedMedia = {
  title = "",
  artist = "",
  album = "",
  currentTime = 0,
  totalTime = 0,
  isPlaying = false,
  thumbnail = ""
}

local function formatTime(seconds)
  if not seconds or seconds < 0 then return "--:--" end
  local mins = math.floor(seconds / 60)
  local secs = math.floor(seconds % 60)
  return string.format("%d:%02d", mins, secs)
end

-- Send media command to the app via UDP
local json = dofile("lua/vehicle/controller/dkjson.lua")

local function sendMediaCommand(command)
  if not hasSocket or not udp then
    dlog('W', "mediaInfoGE", "Cannot send command - UDP module not available")
    return
  end

  local payload = json.encode({command = command})
  local success, err = udp:send(payload)
  if success then
    dlog('I', "mediaInfoGE", "Media command '" .. command .. "' sent via UDP")
  else
    dlog('W', "mediaInfoGE", "Failed to send media command '" .. command .. "': " .. tostring(err))
  end
end

-- Update throttling
local lastMediaUpdate = 0
local MEDIA_UPDATE_INTERVAL = 0.2 -- 200ms (5Hz) - matches app poll rate

-- Fetch media info from the player vehicle's electrics via core_vehicleBridge
local function updateMediaCache(dt)
  lastMediaUpdate = lastMediaUpdate + dt
  if lastMediaUpdate < MEDIA_UPDATE_INTERVAL then return end
  lastMediaUpdate = 0

  local veh = be:getPlayerVehicle(0)
  if not veh then return end

  local vehId = veh:getId()
  local keys = {
    "mediaTitle", "mediaArtist", "mediaAlbum",
    "mediaCurrentTime", "mediaTotalTime", "mediaIsPlaying", "mediaThumbnail"
  }

  for _, key in ipairs(keys) do
    local val = core_vehicleBridge.getCachedVehicleData(vehId, key)
    if val ~= nil then
      -- Map electrics keys to cachedMedia fields (handle camelCase)
      local field = key:gsub("^media", "")
      -- Convert first char to lowercase for matching cachedMedia fields
      field = field:sub(1,1):lower() .. field:sub(2)
      cachedMedia[field] = val
    end
  end
end

local function drawMediaWindow()
  imgui.SetNextWindowSize(imgui.ImVec2(420, 0), imgui.Cond_FirstUseEver)

  if not imgui.Begin("BBC Media Info", M.windowOpen, imgui.WindowFlags_NoCollapse) then
    imgui.End()
    return
  end

  updateMediaCache(imgui.GetIO().DeltaTime)

  local title = cachedMedia.title or ""
  local artist = cachedMedia.artist or ""
  local album = cachedMedia.album or ""
  local currentTime = cachedMedia.currentTime or 0
  local totalTime = cachedMedia.totalTime or 0
  local isPlaying = cachedMedia.isPlaying or false
  local thumbnail = cachedMedia.thumbnail or ""

  -- Display media info
  imgui.Text("Now Playing")
  imgui.Separator()

  imgui.Text("Title:")
  imgui.SameLine()
  imgui.TextColored(imgui.ImVec4(1, 1, 0.8, 1), title ~= "" and title or "No media playing")

  imgui.Text("Artist:")
  imgui.SameLine()
  imgui.TextColored(imgui.ImVec4(0.8, 1, 0.8, 1), artist ~= "" and artist or "Unknown")

  imgui.Text("Album:")
  imgui.SameLine()
  imgui.TextColored(imgui.ImVec4(0.8, 0.8, 1, 1), album ~= "" and album or "Unknown")

  imgui.Spacing()
  imgui.Separator()

  -- Progress bar
  if totalTime and totalTime > 0 then
    local progress = math.min((currentTime or 0) / totalTime, 1.0)
    local timeStr = formatTime(currentTime or 0) .. " / " .. formatTime(totalTime)
    imgui.ProgressBar(progress, imgui.ImVec2(-1, 20), timeStr)
  else
    imgui.ProgressBar(0, imgui.ImVec2(-1, 20), "0:00 / --:--")
  end

  -- Play/pause indicator
  imgui.SameLine()
  if isPlaying then
    imgui.TextColored(imgui.ImVec4(0, 1, 0, 1), "  Playing")
  else
    imgui.TextColored(imgui.ImVec4(1, 0.5, 0, 1), "  Paused")
  end

  imgui.Spacing()
  imgui.Separator()

  -- Thumbnail display
  if thumbnail and thumbnail ~= "" then
    -- Load texture if not cached or new thumbnail
    if not thumbnailTexture or cachedThumbnailBase64 ~= thumbnail then
      thumbnailTexture = loadThumbnailFromBase64(thumbnail)
    end

    imgui.Text("Album Art:")
    imgui.SameLine()

    if thumbnailTexture then
      -- Display the album art image (max 200px wide)
      local size = imgui.ImVec2(150, 150)
      imgui.Image(thumbnailTexture:getID(), size)
    else
      imgui.TextColored(imgui.ImVec4(1, 0.5, 0, 1), "Failed to load image")
    end
  else
    imgui.TextColored(imgui.ImVec4(0.6, 0.6, 0.6, 1), "No album art")
  end

  imgui.Spacing()
  imgui.Separator()

  -- Media controls
  imgui.Text("Controls")
  imgui.Spacing()

  if imgui.Button("<< Previous", imgui.ImVec2(120, 30)) then
    sendMediaCommand("previous")
  end
  imgui.SameLine()
  if isPlaying then
    if imgui.Button("|| Pause", imgui.ImVec2(120, 30)) then
      sendMediaCommand("play_pause")
    end
  else
    if imgui.Button("> Play", imgui.ImVec2(120, 30)) then
      sendMediaCommand("play_pause")
    end
  end
  imgui.SameLine()
  if imgui.Button("Next >>", imgui.ImVec2(120, 30)) then
    sendMediaCommand("next")
  end

  imgui.Spacing()
  imgui.Separator()
  imgui.TextColored(imgui.ImVec4(0.6, 0.6, 1, 1), "Data from BBC Stereo app via electrics")
  imgui.TextColored(imgui.ImVec4(0.6, 0.6, 1, 1), "UDP commands -> app on port 5001")

  imgui.End()
end

local function onUpdate(dtReal, dtSim, dtRaw)
  if M.windowOpen[0] then
    drawMediaWindow()
  end
end

M.onExtensionLoaded = function()
  dlog('I', "mediaInfoGE", "GE extension loaded - Media info display ready")
end

M.onExtensionUnloaded = function()
  dlog('D', "mediaInfoGE", "GE extension unloaded")
end

M.onUpdate = onUpdate

M.toggleMediaMenu = function() M.windowOpen[0] = not M.windowOpen[0] end
M.setMediaMenuOpen = function(open) M.windowOpen[0] = open and true or false end

return M