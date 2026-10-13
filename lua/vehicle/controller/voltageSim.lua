-- voltageSim.lua
-- Shared voltage gauge simulation for the subwoofer controllers.
-- Base voltage follows RPM; hard subwoofer hits cause a temporary voltage
-- drop (battery sag). Both subwooferController and subwooferControllerWs
-- load this and feed it (dt, subValue, rpm) once per update tick.

local M = {}

-- Oscillation/flicker speed for the idle band (400-1000 rpm)
local VOLTAGE_OSCILLATION_SPEED = 0.5 -- Hz

-- Voltage drop thresholds
local VOLTAGE_DROP_THRESHOLD = 0.15
local VOLTAGE_DROP_AMOUNT = 0.5
local VOLTAGE_DROP_RANDOM = 0.3
local VOLTAGE_DROP_DURATION = 2.0 -- seconds

local baseVoltage = 12.5
local voltage = 12.5
local voltageDropActive = false
local voltageDropTimer = 0
local timeAccumulator = 0

-- Returns the current voltage. Call once per controller update tick with the
-- real frame dt, the subwoofer level (electrics.values.subWoofer) and the
-- engine rpm.
local function update(dt, subValue, rpm)
  -- ==============================
  -- Base voltage logic
  -- ==============================
  if rpm < 400 then
    baseVoltage = 12.5
  elseif rpm >= 1000 and rpm <= 5000 then
    local t = (rpm - 1000) / (5000 - 1000)
    baseVoltage = 12.5 + t * (17.5 - 12.5)
  elseif rpm > 5000 then
    baseVoltage = 17.5
  else
    timeAccumulator = timeAccumulator + dt
    baseVoltage = 12.5 + 0.25 * math.sin(timeAccumulator * 2 * math.pi * VOLTAGE_OSCILLATION_SPEED)
  end

  -- ==============================
  -- Voltage drop from subwoofer
  -- ==============================
  if math.abs(subValue) > VOLTAGE_DROP_THRESHOLD then
    if not voltageDropActive then
      voltageDropActive = true
      voltageDropTimer = 0
      voltage = baseVoltage - VOLTAGE_DROP_AMOUNT - math.random() * VOLTAGE_DROP_RANDOM
    end
  end

  if voltageDropActive then
    voltageDropTimer = voltageDropTimer + dt
    if rpm == 0 then
      voltage = voltage - dt * 0.5
      if voltage < 10 then voltage = 10 end
    else
      if voltageDropTimer >= VOLTAGE_DROP_DURATION then
        voltageDropActive = false
        voltageDropTimer = 0
        voltage = baseVoltage
      end
    end
  else
    voltage = baseVoltage
  end

  return voltage
end

local function reset()
  baseVoltage = 12.5
  voltage = 12.5
  voltageDropActive = false
  voltageDropTimer = 0
  timeAccumulator = 0
end

M.update = update
M.reset = reset
M.getVoltage = function() return voltage end
M.getBaseVoltage = function() return baseVoltage end

return M
