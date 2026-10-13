-- bbcMusicBridge - GE-side bridge between the BBC Universal Screen
-- webview and the invisible "BBC Audio Player" UI app that owns the real
-- <audio> element. The screen can play/pause/seek/load audio via the
-- invisible app; this bridge relays commands and forwards time reports
-- back to the screen so the UI stays in sync.

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
  dlog('I', "bbcMusicBridge", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

local catalog = {} -- { {title, artist, file, cover, loop}, ... }
local isPlaying = false

-- Push catalog to the UI app (called when catalog changes or on init)
local function pushCatalog()
    if not catalog then return end
    local payload = { tracks = catalog }
    obj:queueGameEngineLua("guihooks.trigger('bbcMusicCmd', " .. jsonEncode({cmd="catalog", tracks=catalog}) .. ")")
end

-- Forward audio status reports from the UI app to the screen
local function audioReport(currentTime, duration, paused, ended, channel)
    if not extensions.mediaBridgeGE then return end
    local media = extensions.mediaBridgeGE.getMediaInfo()
    media.currentTime = tonumber(currentTime) or 0
    media.totalTime = tonumber(duration) or 0
    media.isPlaying = not (paused == "true" or paused == true)
    -- Note: channel is the track identifier, could be used for track-specific updates
end

-- Handle commands from the screen webview
local function handleScreenCmd(cmd, args)
    if cmd == "catalog" then
        pushCatalog()
    end
end

M.onUpdate = function(dt)
    -- Could add periodic sync here if needed
end

M.handleScreenCmd = handleScreenCmd
M.audioReport = audioReport
M.pushCatalog = pushCatalog

return M