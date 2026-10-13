-- Vehicle extension to export media info to electrics (if needed)
-- The media info is already written to electrics by subwooferController,
-- this is just a placeholder in case we need vehicle-side logic later.

local M = {}

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "mediaInfo", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

M.onExtensionLoaded = function()
  dlog('I', "mediaInfo", "Vehicle extension loaded")
end

M.onExtensionUnloaded = function()
  dlog('D', "mediaInfo", "Vehicle extension unloaded")
end

return M