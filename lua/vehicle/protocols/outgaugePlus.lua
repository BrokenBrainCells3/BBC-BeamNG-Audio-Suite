-- Custom OutGauge protocol with camera data (SINGLE PRECISION)
-- Extends the base outgauge protocol to include camera position and rotation
-- Uses float (32-bit) for all position data

local M = {}

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "outgaugePlus", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

local hasShiftLights = false
local function init()
  local shiftLightControllers = controller.getControllersByType("shiftLights")
  hasShiftLights = shiftLightControllers and #shiftLightControllers > 0
end

local function reset() end
local function getAddress()        return settings.getValue("protocols_outgauge_address") end
local function getPort()           return settings.getValue("protocols_outgauge_port") end
local function getMaxUpdateRate()  return settings.getValue("protocols_outgauge_maxUpdateRate") end

local function isPhysicsStepUsed()
  return false -- use graphics step
end

local function getStructDefinition()
  return [[
    char           magic[4];        // Magic marker: "BNGC" (BeamNG Camera)
    unsigned       time;
    char           car[4];
    unsigned short flags;
    char           gear;
    char           plid;
    float          speed;
    float          rpm;
    float          turbo;
    float          engTemp;
    float          fuel;
    float          oilPressure;
    float          oilTemp;
    unsigned       dashLights;
    unsigned       showLights;
    float          throttle;
    float          brake;
    float          clutch;
    char           display1[16];
    char           display2[16];
    int            id;
    
    // Camera Data (float for orientation is sufficient)
    float          camLocalX;
    float          camLocalY;
    float          camLocalZ;
    float          camLocalPitch;
    float          camLocalYaw;
    float          camLocalRoll;
    float          camLocalQuatX;
    float          camLocalQuatY;
    float          camLocalQuatZ;
    float          camLocalQuatW;
    float          camLocalForwardX;
    float          camLocalForwardY;
    float          camLocalForwardZ;
    float          camGlobalX;
    float          camGlobalY;
    float          camGlobalZ;
    float          camGlobalPitch;
    float          camGlobalYaw;
    float          camGlobalRoll;
    float          camGlobalQuatX;
    float          camGlobalQuatY;
    float          camGlobalQuatZ;
    float          camGlobalQuatW;
    float          camGlobalForwardX;
    float          camGlobalForwardY;
    float          camGlobalForwardZ;
    
    // Vehicle Data - SINGLE PRECISION
    float          vehPosX;
    float          vehPosY;
    float          vehPosZ;
    float          vehQuatX;
    float          vehQuatY;
    float          vehQuatZ;
    float          vehQuatW;
    
    // Speaker Positions (8 speakers × 3 floats = 24 floats) - SINGLE PRECISION
    // Driver Side
    float          speaker_driverspk_x;
    float          speaker_driverspk_y;
    float          speaker_driverspk_z;
    float          speaker_driverrearspk_x;
    float          speaker_driverrearspk_y;
    float          speaker_driverrearspk_z;
    float          speaker_driverrear2spk_x;
    float          speaker_driverrear2spk_y;
    float          speaker_driverrear2spk_z;
    float          speaker_driversubwoofer_x;
    float          speaker_driversubwoofer_y;
    float          speaker_driversubwoofer_z;
    // Passenger Side
    float          speaker_passengerspk_x;
    float          speaker_passengerspk_y;
    float          speaker_passengerspk_z;
    float          speaker_passengerrearspk_x;
    float          speaker_passengerrearspk_y;
    float          speaker_passengerrearspk_z;
    float          speaker_passengerrear2spk_x;
    float          speaker_passengerrear2spk_y;
    float          speaker_passengerrear2spk_z;
    float          speaker_passengersubwoofer_x;
    float          speaker_passengersubwoofer_y;
    float          speaker_passengersubwoofer_z;
    // Speaker Validity Flags (packed into unsigned int)
    unsigned       speaker_valid_flags;  // Bit flags: bit 0=driverspk, 1=driverrearspk, etc.
    
    // EQ Bands (15 bands, range -16 to 16)
    float          eqband1;   // 60 Hz
    float          eqband2;   // 150 Hz
    float          eqband3;   // 300 Hz
    float          eqband4;   // 600 Hz
    float          eqband5;   // 1 kHz
    float          eqband6;   // 2.4 kHz
    float          eqband7;   // 4 kHz
    float          eqband8;   // 6 kHz
    float          eqband9;   // 8 kHz
    float          eqband10;  // 10 kHz
    float          eqband11;  // 12 kHz
    float          eqband12;  // 14 kHz
    float          eqband13;  // 16 kHz
    float          eqband14;  // 18 kHz
    float          eqband15;  // 20 kHz
    
    // Master Volume (range -16 to 16)
    float          volume;

    // Camera Mode
    char           bbccamMode[16];  // BBC camera mode name from electrics.values.bbccamMode

    // Door & Coupler Status (4 corners: FL, FR, RL, RR)
    float          door_FL_coupler_notAttached;  // Front-left coupler (0=attached, 1=not attached)
    float          door_FR_coupler_notAttached;  // Front-right coupler (0=attached, 1=not attached)
    float          door_RL_coupler_notAttached;  // Rear-left coupler (0=attached, 1=not attached)
    float          door_RR_coupler_notAttached;  // Rear-right coupler (0=attached, 1=not attached)
unsigned       couplerAttachedFlags;  // Bit flags: bit 0=FL, bit 1=FR, bit 2=RL, bit 3=RR (1=attached)

    // Speaker Blocked Flags (raycast from camera to each speaker is occluded)
    unsigned       speaker_blocked_flags;  // Same bit order as speaker_valid_flags (1=blocked by vehicle body)
    unsigned       speaker_static_blocked_flags;  // Same bit order (1=blocked by static object, echo sound)

    // Tweeter Positions (2 optional tweeters × 3 floats, appended at end)
    // Kept at the end so appending never shifts the offsets above.
    float          speaker_drivertwt_x;
    float          speaker_drivertwt_y;
    float          speaker_drivertwt_z;
    float          speaker_passengertwt_x;
    float          speaker_passengertwt_y;
    float          speaker_passengertwt_z;
  ]]
end

-- OG_x - bits for flags
local OG_SHIFT =     1
local OG_CTRL  =     2
local OG_TURBO =  8192
local OG_KM    = 16384
local OG_BAR   = 32768

-- DL_x - bits for dashLights and showLights
local DL_SHIFT        = 2 ^ 0
local DL_FULLBEAM     = 2 ^ 1
local DL_HANDBRAKE    = 2 ^ 2
local DL_PITSPEED     = 2 ^ 3
local DL_TC           = 2 ^ 4
local DL_SIGNAL_L     = 2 ^ 5
local DL_SIGNAL_R     = 2 ^ 6
local DL_SIGNAL_ANY   = 2 ^ 7
local DL_OILWARN      = 2 ^ 8
local DL_BATTERY      = 2 ^ 9
local DL_ABS          = 2 ^ 10
local DL_SPARE        = 2 ^ 11

local dlogCounter = 0

local function fillStruct(o, dtSim)
  if not electrics.values.camGlobalZ then
    return
  end

  -- Set magic marker
  o.magic = "BNGC"  -- BeamNG Camera protocol identifier
  
  -- Base vehicle data
  o.time = 0
  o.car = "beam"
  o.flags = OG_KM + OG_BAR + (electrics.values.turboBoost and OG_TURBO or 0)
  o.gear = (electrics.values.gearIndex or 0) + 1
  o.plid = 0
  o.speed = electrics.values.wheelspeed or electrics.values.airspeed
  o.rpm = electrics.values.rpm or 0
  o.turbo = (electrics.values.turboBoost or 0) / 14.504
  o.engTemp = electrics.values.watertemp or 0
  o.fuel = electrics.values.fuel or 0
  o.oilPressure = 0
  o.oilTemp = electrics.values.oiltemp or 0

  -- Lights
  o.dashLights = 0
  o.showLights = 0
  o.dashLights = bit.bor(o.dashLights, DL_FULLBEAM ) if electrics.values.highbeam      ~= 0 then o.showLights = bit.bor(o.showLights, DL_FULLBEAM ) end
  o.dashLights = bit.bor(o.dashLights, DL_HANDBRAKE) if electrics.values.parkingbrake  ~= 0 then o.showLights = bit.bor(o.showLights, DL_HANDBRAKE) end
  o.dashLights = bit.bor(o.dashLights, DL_SIGNAL_L ) if electrics.values.signal_L      ~= 0 then o.showLights = bit.bor(o.showLights, DL_SIGNAL_L ) end
  o.dashLights = bit.bor(o.dashLights, DL_SIGNAL_R ) if electrics.values.signal_R      ~= 0 then o.showLights = bit.bor(o.showLights, DL_SIGNAL_R ) end
  if electrics.values.hasABS then
    o.dashLights = bit.bor(o.dashLights, DL_ABS    ) if electrics.values.absActive     ~= 0 then o.showLights = bit.bor(o.showLights, DL_ABS      ) end
  end
  o.dashLights = bit.bor(o.dashLights, DL_OILWARN  ) if electrics.values.oil           ~= 0 then o.showLights = bit.bor(o.showLights, DL_OILWARN  ) end
  o.dashLights = bit.bor(o.dashLights, DL_BATTERY  ) if electrics.values.engineRunning == 0 then o.showLights = bit.bor(o.showLights, DL_BATTERY  ) end
  if electrics.values.hasESC then
    o.dashLights = bit.bor(o.dashLights, DL_TC     ) if electrics.values.esc           ~= 0
                                                     or electrics.values.tcs           ~= 0 then o.showLights = bit.bor(o.showLights, DL_TC       ) end
  end
  if hasShiftLights then
    o.dashLights = bit.bor(o.dashLights, DL_SHIFT  ) if electrics.values.shouldShift        then o.showLights = bit.bor(o.showLights, DL_SHIFT    ) end
  end

  o.throttle = electrics.values.throttle
  o.brake = electrics.values.brake
  o.clutch = electrics.values.clutch
  o.display1 = ""
  o.display2 = ""
  o.id = 0

  -- Camera Data (provided by vehicle extension)
  if extensions.cameraOutgauge then
    local camData = extensions.cameraOutgauge.getCameraData()
    if camData then
      o.camLocalX = camData.localPos.x or 0
      o.camLocalY = camData.localPos.y or 0
      o.camLocalZ = camData.localPos.z or 0
      o.camLocalPitch = camData.localRot.pitch or 0
      o.camLocalYaw = camData.localRot.yaw or 0
      o.camLocalRoll = camData.localRot.roll or 0
      o.camLocalQuatX = camData.localRotQuat.x or 0
      o.camLocalQuatY = camData.localRotQuat.y or 0
      o.camLocalQuatZ = camData.localRotQuat.z or 0
      o.camLocalQuatW = camData.localRotQuat.w or 0
      o.camLocalForwardX = camData.localForward.x or 0
      o.camLocalForwardY = camData.localForward.y or 0
      o.camLocalForwardZ = camData.localForward.z or 0
      o.camGlobalX = camData.globalPos.x or 0
      o.camGlobalY = camData.globalPos.y or 0
      o.camGlobalZ = camData.globalPos.z or 0
      o.camGlobalPitch = camData.globalRot.pitch or 0
      o.camGlobalYaw = camData.globalRot.yaw or 0
      o.camGlobalRoll = camData.globalRot.roll or 0
      o.camGlobalQuatX = camData.globalRotQuat.x or 0
      o.camGlobalQuatY = camData.globalRotQuat.y or 0
      o.camGlobalQuatZ = camData.globalRotQuat.z or 0
      o.camGlobalQuatW = camData.globalRotQuat.w or 0
      o.camGlobalForwardX = camData.globalForward.x or 0
      o.camGlobalForwardY = camData.globalForward.y or 0
      o.camGlobalForwardZ = camData.globalForward.z or 0

      -- Debug first time
      if not M._dlogged then
        M._dlogged = true
        dlog('I', "outgaugePlus", string.format("Sending camera - Local: (%.2f, %.2f, %.2f) Fwd: (%.3f, %.3f, %.3f) | Global: (%.2f, %.2f, %.2f) Fwd: (%.3f, %.3f, %.3f)",
          o.camLocalX, o.camLocalY, o.camLocalZ,
          o.camLocalForwardX, o.camLocalForwardY, o.camLocalForwardZ,
          o.camGlobalX, o.camGlobalY, o.camGlobalZ,
          o.camGlobalForwardX, o.camGlobalForwardY, o.camGlobalForwardZ))
      end
    end

    -- Vehicle Data (position and rotation quaternion)
    extensions.cameraOutgauge.updateVehicleData()
    local vehData = extensions.cameraOutgauge.getVehicleData()
    if vehData then
      o.vehPosX = vehData.pos.x or 0
      o.vehPosY = vehData.pos.y or 0
      o.vehPosZ = vehData.pos.z or 0
      o.vehQuatX = vehData.rotQuat.x or 0
      o.vehQuatY = vehData.rotQuat.y or 0
      o.vehQuatZ = vehData.rotQuat.z or 0
      o.vehQuatW = vehData.rotQuat.w or 0
    end
  else
    dlog('W', "outgaugePlus", "cameraOutgauge extension NOT FOUND!")
  end

  -- Speaker Positions (from electric values exported by speakerTrackerGE)
  -- Driver Side
  o.speaker_driverspk_x = electrics.values.speaker_abs_driverspk_x or 0
  o.speaker_driverspk_y = electrics.values.speaker_abs_driverspk_y or 0
  o.speaker_driverspk_z = electrics.values.speaker_abs_driverspk_z or 0
  o.speaker_driverrearspk_x = electrics.values.speaker_abs_driverrearspk_x or 0
  o.speaker_driverrearspk_y = electrics.values.speaker_abs_driverrearspk_y or 0
  o.speaker_driverrearspk_z = electrics.values.speaker_abs_driverrearspk_z or 0
  o.speaker_driverrear2spk_x = electrics.values.speaker_abs_driverrear2spk_x or 0
  o.speaker_driverrear2spk_y = electrics.values.speaker_abs_driverrear2spk_y or 0
  o.speaker_driverrear2spk_z = electrics.values.speaker_abs_driverrear2spk_z or 0
  o.speaker_driversubwoofer_x = electrics.values.speaker_abs_driversubwoofer_x or 0
  o.speaker_driversubwoofer_y = electrics.values.speaker_abs_driversubwoofer_y or 0
  o.speaker_driversubwoofer_z = electrics.values.speaker_abs_driversubwoofer_z or 0

  -- Passenger Side
  o.speaker_passengerspk_x = electrics.values.speaker_abs_passengerspk_x or 0
  o.speaker_passengerspk_y = electrics.values.speaker_abs_passengerspk_y or 0
  o.speaker_passengerspk_z = electrics.values.speaker_abs_passengerspk_z or 0
  o.speaker_passengerrearspk_x = electrics.values.speaker_abs_passengerrearspk_x or 0
  o.speaker_passengerrearspk_y = electrics.values.speaker_abs_passengerrearspk_y or 0
  o.speaker_passengerrearspk_z = electrics.values.speaker_abs_passengerrearspk_z or 0
  o.speaker_passengerrear2spk_x = electrics.values.speaker_abs_passengerrear2spk_x or 0
  o.speaker_passengerrear2spk_y = electrics.values.speaker_abs_passengerrear2spk_y or 0
  o.speaker_passengerrear2spk_z = electrics.values.speaker_abs_passengerrear2spk_z or 0
  o.speaker_passengersubwoofer_x = electrics.values.speaker_abs_passengersubwoofer_x or 0
  o.speaker_passengersubwoofer_y = electrics.values.speaker_abs_passengersubwoofer_y or 0
  o.speaker_passengersubwoofer_z = electrics.values.speaker_abs_passengersubwoofer_z or 0

  -- Speaker Validity Flags (pack into bit flags)
  o.speaker_valid_flags = 0
  if electrics.values.speaker_abs_driverspk_valid == 1 then     o.speaker_valid_flags = bit.bor(o.speaker_valid_flags, 1) end
  if electrics.values.speaker_abs_driverrearspk_valid == 1 then o.speaker_valid_flags = bit.bor(o.speaker_valid_flags, 2) end
  if electrics.values.speaker_abs_driverrear2spk_valid == 1 then o.speaker_valid_flags = bit.bor(o.speaker_valid_flags, 4) end
  if electrics.values.speaker_abs_driversubwoofer_valid == 1 then o.speaker_valid_flags = bit.bor(o.speaker_valid_flags, 8) end
  if electrics.values.speaker_abs_passengerspk_valid == 1 then  o.speaker_valid_flags = bit.bor(o.speaker_valid_flags, 16) end
  if electrics.values.speaker_abs_passengerrearspk_valid == 1 then o.speaker_valid_flags = bit.bor(o.speaker_valid_flags, 32) end
  if electrics.values.speaker_abs_passengerrear2spk_valid == 1 then o.speaker_valid_flags = bit.bor(o.speaker_valid_flags, 64) end
  if electrics.values.speaker_abs_passengersubwoofer_valid == 1 then o.speaker_valid_flags = bit.bor(o.speaker_valid_flags, 128) end

  -- Tweeter Validity Flags (bits 8 & 9, same 32-bit field)
  if electrics.values.speaker_abs_drivertwt_valid == 1 then       o.speaker_valid_flags = bit.bor(o.speaker_valid_flags, 256) end
  if electrics.values.speaker_abs_passengertwt_valid == 1 then    o.speaker_valid_flags = bit.bor(o.speaker_valid_flags, 512) end

  -- EQ Bands
  o.eqband1  = electrics.values.eqband1  or 0
  o.eqband2  = electrics.values.eqband2  or 0
  o.eqband3  = electrics.values.eqband3  or 0
  o.eqband4  = electrics.values.eqband4  or 0
  o.eqband5  = electrics.values.eqband5  or 0
  o.eqband6  = electrics.values.eqband6  or 0
  o.eqband7  = electrics.values.eqband7  or 0
  o.eqband8  = electrics.values.eqband8  or 0
  o.eqband9  = electrics.values.eqband9  or 0
  o.eqband10 = electrics.values.eqband10 or 0
  o.eqband11 = electrics.values.eqband11 or 0
  o.eqband12 = electrics.values.eqband12 or 0
  o.eqband13 = electrics.values.eqband13 or 0
  o.eqband14 = electrics.values.eqband14 or 0
  o.eqband15 = electrics.values.eqband15 or 0

  -- Master Volume
  o.volume = electrics.values.volume or 0

  -- BBC Camera Mode
  o.bbccamMode = electrics.values.bbccamMode or ""

  -- Door & Coupler Status (4 corners: FL, FR, RL, RR)
  -- Reads electrics.values.door_FL_coupler_notAttached, etc. (0=attached, 1=not attached)
  o.door_FL_coupler_notAttached = electrics.values.door_FL_coupler_notAttached or 0
  o.door_FR_coupler_notAttached = electrics.values.door_FR_coupler_notAttached or 0
  o.door_RL_coupler_notAttached = electrics.values.door_RL_coupler_notAttached or 0
  o.door_RR_coupler_notAttached = electrics.values.door_RR_coupler_notAttached or 0

  -- Pack coupler attached flags into bit field (1=attached, 0=not attached)
  o.couplerAttachedFlags = 0
  if o.door_FL_coupler_notAttached == 0 then o.couplerAttachedFlags = bit.bor(o.couplerAttachedFlags, 1) end    -- bit 0 = FL
  if o.door_FR_coupler_notAttached == 0 then o.couplerAttachedFlags = bit.bor(o.couplerAttachedFlags, 2) end    -- bit 1 = FR
  if o.door_RL_coupler_notAttached == 0 then o.couplerAttachedFlags = bit.bor(o.couplerAttachedFlags, 4) end    -- bit 2 = RL
  if o.door_RR_coupler_notAttached == 0 then o.couplerAttachedFlags = bit.bor(o.couplerAttachedFlags, 8) end    -- bit 3 = RR

  -- Speaker Blocked Flags (pack into bit flags, same order as speaker_valid_flags)
  o.speaker_blocked_flags = 0
  if electrics.values.speaker_abs_driverspk_blocked == 1 then     o.speaker_blocked_flags = bit.bor(o.speaker_blocked_flags, 1) end
  if electrics.values.speaker_abs_driverrearspk_blocked == 1 then o.speaker_blocked_flags = bit.bor(o.speaker_blocked_flags, 2) end
  if electrics.values.speaker_abs_driverrear2spk_blocked == 1 then o.speaker_blocked_flags = bit.bor(o.speaker_blocked_flags, 4) end
  if electrics.values.speaker_abs_driversubwoofer_blocked == 1 then o.speaker_blocked_flags = bit.bor(o.speaker_blocked_flags, 8) end
  if electrics.values.speaker_abs_passengerspk_blocked == 1 then  o.speaker_blocked_flags = bit.bor(o.speaker_blocked_flags, 16) end
  if electrics.values.speaker_abs_passengerrearspk_blocked == 1 then o.speaker_blocked_flags = bit.bor(o.speaker_blocked_flags, 32) end
  if electrics.values.speaker_abs_passengerrear2spk_blocked == 1 then o.speaker_blocked_flags = bit.bor(o.speaker_blocked_flags, 64) end
  if electrics.values.speaker_abs_passengersubwoofer_blocked == 1 then o.speaker_blocked_flags = bit.bor(o.speaker_blocked_flags, 128) end

  -- Tweeter Blocked Flags (bits 8 & 9)
  if electrics.values.speaker_abs_drivertwt_blocked == 1 then       o.speaker_blocked_flags = bit.bor(o.speaker_blocked_flags, 256) end
  if electrics.values.speaker_abs_passengertwt_blocked == 1 then    o.speaker_blocked_flags = bit.bor(o.speaker_blocked_flags, 512) end

  -- Speaker Static Blocked Flags (1 = occluded by a static object -> app applies echo)
  o.speaker_static_blocked_flags = 0
  if electrics.values.speaker_abs_driverspk_static_blocked == 1 then     o.speaker_static_blocked_flags = bit.bor(o.speaker_static_blocked_flags, 1) end
  if electrics.values.speaker_abs_driverrearspk_static_blocked == 1 then o.speaker_static_blocked_flags = bit.bor(o.speaker_static_blocked_flags, 2) end
  if electrics.values.speaker_abs_driverrear2spk_static_blocked == 1 then o.speaker_static_blocked_flags = bit.bor(o.speaker_static_blocked_flags, 4) end
  if electrics.values.speaker_abs_driversubwoofer_static_blocked == 1 then o.speaker_static_blocked_flags = bit.bor(o.speaker_static_blocked_flags, 8) end
  if electrics.values.speaker_abs_passengerspk_static_blocked == 1 then  o.speaker_static_blocked_flags = bit.bor(o.speaker_static_blocked_flags, 16) end
  if electrics.values.speaker_abs_passengerrearspk_static_blocked == 1 then o.speaker_static_blocked_flags = bit.bor(o.speaker_static_blocked_flags, 32) end
  if electrics.values.speaker_abs_passengerrear2spk_static_blocked == 1 then o.speaker_static_blocked_flags = bit.bor(o.speaker_static_blocked_flags, 64) end
  if electrics.values.speaker_abs_passengersubwoofer_static_blocked == 1 then o.speaker_static_blocked_flags = bit.bor(o.speaker_static_blocked_flags, 128) end

  -- Tweeter Static Blocked Flags (bits 8 & 9)
  if electrics.values.speaker_abs_drivertwt_static_blocked == 1 then       o.speaker_static_blocked_flags = bit.bor(o.speaker_static_blocked_flags, 256) end
  if electrics.values.speaker_abs_passengertwt_static_blocked == 1 then    o.speaker_static_blocked_flags = bit.bor(o.speaker_static_blocked_flags, 512) end

  -- Tweeter Positions (optional, appended at the end of the packet)
  o.speaker_drivertwt_x = electrics.values.speaker_abs_drivertwt_x or 0
  o.speaker_drivertwt_y = electrics.values.speaker_abs_drivertwt_y or 0
  o.speaker_drivertwt_z = electrics.values.speaker_abs_drivertwt_z or 0
  o.speaker_passengertwt_x = electrics.values.speaker_abs_passengertwt_x or 0
  o.speaker_passengertwt_y = electrics.values.speaker_abs_passengertwt_y or 0
  o.speaker_passengertwt_z = electrics.values.speaker_abs_passengertwt_z or 0
end

M.init = init
M.reset = reset
M.getAddress = getAddress
M.getPort = getPort
M.getMaxUpdateRate = getMaxUpdateRate
M.getStructDefinition = getStructDefinition
M.fillStruct = fillStruct
M.isPhysicsStepUsed = isPhysicsStepUsed

return M