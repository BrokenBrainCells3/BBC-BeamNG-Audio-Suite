-- This Source Code Form is subject to the terms of the bCDDL, v. 1.1.
-- If a copy of the bCDDL was not distributed with this
-- file, You can obtain one at http://beamng.com/bCDDL-1.1.txt

local M = {}
M.type = "auxiliary"
M.relevantDevice = nil

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "tv2", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

local htmlTexture = require("htmlTexture")

local gaugesScreenName = nil
local htmlPath = nil
local updateFPS = 2000

local function updateGFX(dt)
end

local function init(jbeamData)
  gaugesScreenName = jbeamData.materialName
  htmlPath = jbeamData.htmlPath
  local width = jbeamData.textureWidth or 1920
  local height = jbeamData.textureHeight or 1080

  if not gaugesScreenName then
    dlog("E", "tv2", "No htmlPath in jbeam")
    M.updateGFX = nop
  else
    if htmlPath then
      htmlTexture.create(gaugesScreenName, htmlPath, width, height, updateFPS, "automatic")
    else
dlog("E", "tv2", "No material name (screenId) in jbeam")
      M.updateGFX = nop
    end
  end
end

M.init = init
M.updateGFX = updateGFX

return M
