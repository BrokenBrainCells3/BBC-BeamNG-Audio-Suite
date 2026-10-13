--[[ 
subwooferGauge.lua
BeamNG controller
Reads electrics.values set by subwooferController.lua and pushes to HTML
]]--

local M = {}
M.type = "controller"

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "subwooferGauge", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

local gaugeHTMLTexture
local dataCallback = nil

-- Track current values
local currentValues = {
    sub    = 0,
    bass   = 0,
    mid    = 0,
    treble = 0,
    volts  = 12.5
}

-- Safe getter
local function safeNumber(v)
    return (v ~= nil and v) or 0
end

-- Called every graphics tick
local function updateGFX(dt)
    -- ensure electrics.values exist
    if not electrics then electrics = {} end
    if not electrics.values then electrics.values = {} end

    -- read values from electrics written by your other lua
    currentValues.sub    = safeNumber(electrics.values.subWoofer)
    currentValues.bass   = safeNumber(electrics.values.bass)
    currentValues.mid    = safeNumber(electrics.values.mid)
    currentValues.treble = safeNumber(electrics.values.treble)
    currentValues.volts  = safeNumber(electrics.values.volts, 12.5)

    -- debug log
    if debugLogging then
        dlog('D', "subwooferGauge", string.format(
            "[subwooferGauge] Sub=%.3f | Bass=%.3f | Mid=%.3f | Treble=%.3f | Volts=%.2f",
            currentValues.sub, currentValues.bass, currentValues.mid, currentValues.treble, currentValues.volts
        ))
    end

    -- send to HTML gauge if available
    if gaugeHTMLTexture then
        gaugeHTMLTexture:callJS("updateData", {{
            electrics = {
                subWoofer = currentValues.sub,
                bass      = currentValues.bass,
                mid       = currentValues.mid,
                treble    = currentValues.treble,
                volts     = currentValues.volts
            }
        }})
    end

    -- optional Lua callback
    if dataCallback then
        dataCallback(currentValues)
    end
end

local function setupGaugeData(properties, htmlTexture, callback)
    gaugeHTMLTexture = htmlTexture
    dataCallback     = callback
end

local function init(jbeamData)
    dlog('I', "subwooferGauge", "Module initialized - listening for RadioNixon audio data")
end

M.init = init
M.updateGFX = updateGFX
M.setupGaugeData = setupGaugeData

return M
