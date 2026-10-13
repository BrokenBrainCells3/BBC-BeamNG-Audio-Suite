local M = {}

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "screenCalibration", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

local USER_DIR = "settings/bbc_stereo/calibrations/"
local PRESET_DIR = "mods/unpacked/bbc_stereo_0.6/presets/calibrations/"

local function sanitize(name)
  return tostring(name or ""):gsub("[^%w%-_]", "_")
end

local function pathFor(dir, vehName)
  return dir .. sanitize(vehName) .. ".json"
end

local function cleanAnchor(a)
  if type(a) ~= "table" then return nil end
  local function vec(v)
    if type(v) ~= "table" then return nil end
    return { x = tonumber(v.x) or 0, y = tonumber(v.y) or 0, z = tonumber(v.z) or 0 }
  end
  local function vec3o(v)
    if type(v) ~= "table" then return nil end
    return { x = tonumber(v.x) or 0, y = tonumber(v.y) or 0, z = tonumber(v.z) or 0 }
  end
  local function cleanPlane(p)
    if not p then return nil end
    return {
      pos = vec3o(p.pos),
      rot = vec3o(p.rot),
      width = tonumber(p.width) or 0.2,
      height = tonumber(p.height) or 0.2,
    }
  end
  local function cleanBox(b)
    if not b then return nil end
    return {
      pos = vec3o(b.pos),
      rot = vec3o(b.rot),
      size = b.size and vec3o(b.size) or nil,
      translateX = tonumber(b.translateX) or 0,
      translateY = tonumber(b.translateY) or 0,
    }
  end
  local function cleanTrigger(t)
    if not t then return nil end
    return {
      pos = vec3o(t.pos),
      size = vec3o(t.size),
      rot = vec3o(t.rot),
    }
  end
  local out = {}
  if a.refPlanes then
    out.refPlanes = {}
    for id, p in pairs(a.refPlanes) do
      out.refPlanes[id] = cleanPlane(p)
    end
  end
  if a.boxes then
    out.boxes = {}
    for _, b in ipairs(a.boxes) do
      table.insert(out.boxes, cleanBox(b))
    end
  end
  if a.triggers then
    out.triggers = {}
    for _, t in ipairs(a.triggers) do
      table.insert(out.triggers, cleanTrigger(t))
    end
  end
  return out
end

function M.save(vehName, anchor)
  local clean = cleanAnchor(anchor)
  if not vehName or not clean then return false end
  local path = pathFor(USER_DIR, vehName)
  return jsonWriteFile(path, clean, true) ~= false
end

function M.load(vehName)
  if not vehName or vehName == "" then return nil end
  local userPath = pathFor(USER_DIR, vehName)
  local data = jsonReadFile(userPath)
  if data then return cleanAnchor(data), "user" end
  local presetPath = pathFor(PRESET_DIR, vehName)
  data = jsonReadFile(presetPath)
  if data then return cleanAnchor(data), "preset" end
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