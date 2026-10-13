local M = {}

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "lightingCalibration", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

local USER_DIR = "settings/bbc_stereo/lighting/"
local PRESET_DIR = "mods/unpacked/bbc_stereo_0.6/presets/lighting/"

local function sanitize(name)
  return tostring(name or ""):gsub("[^%w%-_]", "_")
end

local function pathFor(dir, vehName)
  return dir .. sanitize(vehName) .. ".json"
end

local function cleanConfig(cfg)
  if type(cfg) ~= "table" then return nil end
  local out = {
    zones = cfg.zones or {},
    brightness = cfg.brightness or 0.75,
    customColor = cfg.customColor or {r = 255, g = 255, b = 255},
    dashboardColor = cfg.dashboardColor or {r = 255, g = 255, b = 255},
    mode = cfg.mode or "static"
  }
  return out
end

function M.save(vehName, cfg)
  local clean = cleanConfig(cfg)
  if not vehName or not clean then return false end
  local path = pathFor(USER_DIR, vehName)
  FS:directoryCreate(USER_DIR, true)
  return jsonWriteFile(path, clean, true) ~= false
end

function M.load(vehName)
  if not vehName or vehName == "" then return nil end
  local userPath = pathFor(USER_DIR, vehName)
  local data = jsonReadFile(userPath)
  if data then return cleanConfig(data), "user" end
  local presetPath = pathFor(PRESET_DIR, vehName)
  data = jsonReadFile(presetPath)
  if data then return cleanConfig(data), "preset" end
  return nil
end

function M.delete(vehName)
  if not vehName then return false end
  local path = pathFor(USER_DIR, vehName)
  if FS and FS.fileExists and FS:fileExists(path) then
    return FS:removeFile(path)
  end
  return false
end

local function gather(dir, kind)
  local out = {}
  if not (FS and FS.findFiles) then return out end
  local files = FS:findFiles(dir, "*.json", 0, false, false) or {}
  for _, f in ipairs(files) do
    local name = tostring(f):match("([^/\\]+)%.json$")
    if name then out[#out+1] = { type = kind, name = name, path = f } end
  end
  return out
end

function M.list()
  local out = {}
  for _, e in ipairs(gather(USER_DIR, "user")) do out[#out+1] = e end
  for _, e in ipairs(gather(PRESET_DIR, "preset")) do out[#out+1] = e end
  return out
end

return M