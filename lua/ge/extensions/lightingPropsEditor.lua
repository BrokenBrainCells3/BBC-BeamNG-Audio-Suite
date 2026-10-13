-- lightingPropsEditor - ImGui editor for vehicle screen lighting prop positions
-- Uses the same approach as vePropTransformer for real-time prop manipulation
-- Saves per-vehicle JSON config to settings/bbc_stereo/lighting_props/

local M = {}

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "lightingPropsEditor", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

M.dependencies = {"ui_imgui"}

local im = ui_imgui
local utils = require('jbeam/utils')

local wndName = "Lighting Props Editor"
local windowOpen = im.BoolPtr(false)
local showPropDebug = im.BoolPtr(false) -- Toggle for prop visualization
local statusMsg = "Ready."
local statusIsError = false
local synced = false

-- Edit state per prop
local propEdit = {}
local propPtrs = {}
local livePropCount = 0

-- Vehicle info
local currentVehId = -1
local currentVehName = nil
local lastUIVehName = nil
local uiLoadedForVehicle = false

-- Cached jbeam-space position of the vehicle's ref node.
-- Prop base translations are in jbeam coordinates, but veh:getPosition() is the
-- ref node's world position, so the ref node's jbeam position must be subtracted
-- before rotating into world space.
local refJbeamPos = nil
local refJbeamVehId = nil

local function getRefJbeamPos(veh)
    local id = veh:getID()
    if refJbeamPos and refJbeamVehId == id then return refJbeamPos end
    refJbeamVehId = id
    refJbeamPos = vec3(0, 0, 0)

    local ok, vd = pcall(function()
        return core_vehicle_manager.getVehicleData(id)
    end)
    local vdata = ok and vd and vd.vdata
    if vdata and vdata.refNodes and vdata.refNodes[0] and vdata.nodes then
        local n = vdata.nodes[vdata.refNodes[0].ref]
        if n and n.pos then
            refJbeamPos = vec3(n.pos.x, n.pos.y, n.pos.z)
        end
    end
    dlog('D', 'lightingPropsEditor', string.format('Ref node jbeam pos: (%.3f, %.3f, %.3f)', refJbeamPos.x, refJbeamPos.y, refJbeamPos.z))
    return refJbeamPos
end

-- Config paths
local function sanitize(name)
    return tostring(name or ""):gsub("[^%w%-_]", "_")
end

local function getConfigPath(vehName)
    return "settings/bbc_stereo/lighting_props/" .. sanitize(vehName) .. ".json"
end

local function getVehName(veh)
    if not veh then return nil end
    if veh.getJBeamFilename then
        local ok, n = pcall(veh.getJBeamFilename, veh)
        if ok and n and n ~= "" then return n end
    end
    return veh.JBeam or veh.jbeam or nil
end

-- Get all lighting props from the vehicle (SPOTLIGHT/POINTLIGHT)
-- Filter to only show props that belong to the mediaInfoScreen/universal screen
-- Match the LIGHT_FUNCS list from lightingController
local MEDIA_SCREEN_LIGHT_FUNCS = {
    ["footwell_l"] = true,
    ["footwell_r"] = true,
    ["footwell_rear_l"] = true,
    ["footwell_rear_r"] = true,
    ["dashboard_lights"] = true,
    ["door_l"] = true,
    ["door_r"] = true,
    ["cupholder"] = true,
    ["storage"] = true,
}

local function getVehicleLightingProps()
    local veh = be:getPlayerVehicle(0)
    if not veh then return nil end
    
    local propsData = veh.data and veh.data.props
    if not propsData then return nil end
    
    local lightProps = {}
    for i = 0, tableSizeC(propsData) - 1 do
        local prop = propsData[i]
        if prop.pid and (prop.mesh == "SPOTLIGHT" or prop.mesh == "POINTLIGHT") then
            -- Only include props that are part of the universal screen lighting
            if MEDIA_SCREEN_LIGHT_FUNCS[prop.func] then
                local pos = {x = 0, y = 0, z = 0}
                local rot = {x = 0, y = 0, z = 0}
                local color = {r = 255, g = 255, b = 255}
                local brightness = 1.0
                local range = 1.0
                local innerAngle = 45
                local outerAngle = 60
                local castShadows = false
                
                if prop.baseTranslation then
                    pos.x = prop.baseTranslation.x or 0
                    pos.y = prop.baseTranslation.y or 0
                    pos.z = prop.baseTranslation.z or 0
                end
                if prop.baseRotation then
                    rot.x = prop.baseRotation.x or 0
                    rot.y = prop.baseRotation.y or 0
                    rot.z = prop.baseRotation.z or 0
                end
                if prop.lightColor then
                    color.r = prop.lightColor.r or 255
                    color.g = prop.lightColor.g or 255
                    color.b = prop.lightColor.b or 255
                end
                if prop.lightBrightness then brightness = prop.lightBrightness end
                if prop.lightRange then range = prop.lightRange end
                if prop.lightInnerAngle then innerAngle = prop.lightInnerAngle end
                if prop.lightOuterAngle then outerAngle = prop.lightOuterAngle end
                if prop.lightCastShadows ~= nil then castShadows = prop.lightCastShadows end
                
                table.insert(lightProps, {
                    func = prop.func,
                    mesh = prop.mesh,
                    pid = prop.pid,
                    pos = pos,
                    rot = rot,
                    color = color,
                    brightness = brightness,
                    range = range,
                    innerAngle = innerAngle,
                    outerAngle = outerAngle,
                    castShadows = castShadows,
                    electric = prop.electric,
                    idRef = prop.idRef,
                    idX = prop.idX,
                    idY = prop.idY,
                    partPath = prop.partPath
                })
            end
        end
    end
    return lightProps
end

-- Called by screenService when vehicle controller sends prop data
local function onLightingPropsData(props)
    dlog('I', 'lightingPropsEditor', 'Received lighting props data from vehicle controller (' .. #props .. ' props)')
    -- Filter to only our media screen lighting props
    local filtered = {}
    for _, prop in ipairs(props) do
        if MEDIA_SCREEN_LIGHT_FUNCS[prop.func] then
            table.insert(filtered, prop)
        end
    end
    dlog('I', 'lightingPropsEditor', 'Filtered to ' .. #filtered .. ' media screen lighting props')
    -- Store for syncFromLive to use
    _G._lightingPropsEditor_vehicleProps = filtered
end

-- Forward declarations for functions used in syncFromLive
local pushProp -- defined below; must be declared before syncFromLive uses it

local function setStatus(msg, isError)
    statusMsg = msg
    statusIsError = isError and true or false
end

-- Sync live props into edit tables, auto-load saved config if exists
local function syncFromLive()
    local veh = be:getPlayerVehicle(0)
    if not veh then 
        setStatus("No vehicle spawned", true)
        return 
    end
    
    currentVehId = veh:getID()
    currentVehName = getVehName(veh)
    
    -- Prefer data from vehicle controller (sent via screenService)
    local props = _G._lightingPropsEditor_vehicleProps
    if not props or #props == 0 then
        -- Fallback: query vehicle directly
        local vehProps = getVehicleLightingProps()
        if vehProps and #vehProps > 0 then
            props = vehProps
        end
    end
    
    if not props or #props == 0 then
        setStatus("No SPOTLIGHT/POINTLIGHT props found on this vehicle", true)
        livePropCount = 0
        propEdit, propPtrs = {}, {}
        return
    end
    
    -- Drop duplicate entries (same pid) in case data was received more than once
    local seen, unique = {}, {}
    for _, prop in ipairs(props) do
        if prop.pid == nil or not seen[prop.pid] then
            if prop.pid ~= nil then seen[prop.pid] = true end
            table.insert(unique, prop)
        end
    end
    props = unique
    refJbeamPos = nil -- force re-read of ref node position
    
    -- Clear and rebuild from vehicle controller data
    propEdit, propPtrs = {}, {}
    livePropCount = #props
    
    for i, prop in ipairs(props) do
        propEdit[i] = deepcopy(prop)
        
        propPtrs[i] = {
            x = im.FloatPtr(prop.pos.x),
            y = im.FloatPtr(prop.pos.y),
            z = im.FloatPtr(prop.pos.z),
            rx = im.FloatPtr(prop.rot.x),
            ry = im.FloatPtr(prop.rot.y),
            rz = im.FloatPtr(prop.rot.z),
            brightness = im.FloatPtr(prop.brightness),
            range = im.FloatPtr(prop.range),
            innerAngle = im.FloatPtr(prop.innerAngle),
            outerAngle = im.FloatPtr(prop.outerAngle),
            castShadows = im.BoolPtr(prop.castShadows),
        }
    end
    
    -- Auto-load saved config if exists (overrides jbeam defaults)
    if currentVehName then
        local data = jsonReadFile(getConfigPath(currentVehName))
        if data and data.props then
            dlog('I', 'lightingPropsEditor', 'Auto-loaded saved config for ' .. currentVehName)
            for i, e in ipairs(propEdit) do
                local saved = data.props[e.func]
                if saved then
                    if saved.pos then
                        e.pos.x, e.pos.y, e.pos.z = saved.pos.x, saved.pos.y, saved.pos.z
                        if propPtrs[i] then
                            propPtrs[i].x[0], propPtrs[i].y[0], propPtrs[i].z[0] = e.pos.x, e.pos.y, e.pos.z
                        end
                    end
                    if saved.rot then
                        e.rot.x, e.rot.y, e.rot.z = saved.rot.x, saved.rot.y, saved.rot.z
                        if propPtrs[i] then
                            propPtrs[i].rx[0], propPtrs[i].ry[0], propPtrs[i].rz[0] = e.rot.x, e.rot.y, e.rot.z
                        end
                    end
                    if saved.brightness then
                        e.brightness = saved.brightness
                        if propPtrs[i] then propPtrs[i].brightness[0] = e.brightness end
                    end
                    if saved.range then
                        e.range = saved.range
                        if propPtrs[i] then propPtrs[i].range[0] = e.range end
                    end
                    if saved.innerAngle then
                        e.innerAngle = saved.innerAngle
                        if propPtrs[i] then propPtrs[i].innerAngle[0] = e.innerAngle end
                    end
                    if saved.outerAngle then
                        e.outerAngle = saved.outerAngle
                        if propPtrs[i] then propPtrs[i].outerAngle[0] = e.outerAngle end
                    end
if saved.castShadows ~= nil then
                    e.castShadows = saved.castShadows
                    if propPtrs[i] then propPtrs[i].castShadows[0] = e.castShadows end
                end
                if saved.color then
                        e.color.r, e.color.g, e.color.b = saved.color.r, saved.color.g, saved.color.b
                    end
                    if saved.flareName then e.flareName = saved.flareName end
                end
            end
            -- Apply loaded values to live props
            for i = 1, livePropCount do pushProp(i) end
            setStatus("Loaded saved config for " .. currentVehName, false)
        else
            setStatus("Synced " .. livePropCount .. " prop(s) from vehicle controller", false)
        end
    end
    
    synced = true
end

-- Push live changes to the actual vehicle prop (like vePropTransformer)
function pushProp(i)
    local veh = be:getPlayerVehicle(0)
    if not veh then return end
    
    local e = propEdit[i]
    local p = propPtrs[i]
    if not e or not p then return end
    
    -- Update edit table from pointers
    e.pos.x, e.pos.y, e.pos.z = p.x[0], p.y[0], p.z[0]
    e.rot.x, e.rot.y, e.rot.z = p.rx[0], p.ry[0], p.rz[0]
    e.brightness = p.brightness[0]
    e.range = p.range[0]
    e.innerAngle = p.innerAngle[0]
    e.outerAngle = p.outerAngle[0]
    e.castShadows = p.castShadows[0]
    
    -- Get the actual prop object and apply position/rotation changes
    local propObj = veh:getProp(e.pid)
    if propObj then
        -- Apply position and rotation (these work on prop object)
        propObj:setBaseTranslation(vec3(e.pos.x, e.pos.y, e.pos.z))
        propObj:setBaseRotation(vec3(e.rot.x, e.rot.y, e.rot.z))
    end
    
    -- Light properties (range, angles, castShadows) are handled by lightingController
    -- via obj:setPropLight() - we need to notify the vehicle controller of the changes
    -- For now, lightingController will pick up the changes on next update
end

-- Save config per vehicle
local function saveConfig()
    if not currentVehName then
        setStatus("No vehicle loaded", true)
        return
    end
    
    local data = {props = {}}
    for i, e in ipairs(propEdit) do
        data.props[e.func] = {
            pos = {x = e.pos.x, y = e.pos.y, z = e.pos.z},
            rot = {x = e.rot.x, y = e.rot.y, z = e.rot.z},
            color = {r = e.color.r, g = e.color.g, b = e.color.b},
            brightness = e.brightness,
            range = e.range,
            innerAngle = e.innerAngle,
            outerAngle = e.outerAngle,
            castShadows = e.castShadows,
            electric = e.electric,
            mesh = e.mesh,
            pid = e.pid
        }
    end
    
    local path = getConfigPath(currentVehName)
    local ok = pcall(jsonWriteFile, path, data, true)
    if ok then
        setStatus("Saved config for " .. currentVehName, false)
        dlog('I', 'lightingPropsEditor', 'Saved lighting props config to ' .. path)
    else
        setStatus("Failed to save config", true)
        dlog('W', 'lightingPropsEditor', 'Failed to write config to ' .. path)
    end
end

-- Load config per vehicle
local function loadConfig()
    if not currentVehName then return end
    
    local data = jsonReadFile(getConfigPath(currentVehName))
    if not data or not data.props then
        dlog('D', 'lightingPropsEditor', 'No saved config for ' .. currentVehName)
        return
    end
    
    dlog('I', 'lightingPropsEditor', 'Loaded lighting props config for ' .. currentVehName)
    
    -- Apply saved values to edit tables
    for i, e in ipairs(propEdit) do
        local saved = data.props[e.func]
        if saved then
            if saved.pos then
                e.pos.x, e.pos.y, e.pos.z = saved.pos.x, saved.pos.y, saved.pos.z
                if propPtrs[i] then
                    propPtrs[i].x[0], propPtrs[i].y[0], propPtrs[i].z[0] = e.pos.x, e.pos.y, e.pos.z
                end
            end
            if saved.rot then
                e.rot.x, e.rot.y, e.rot.z = saved.rot.x, saved.rot.y, saved.rot.z
                if propPtrs[i] then
                    propPtrs[i].rx[0], propPtrs[i].ry[0], propPtrs[i].rz[0] = e.rot.x, e.rot.y, e.rot.z
                end
            end
            if saved.brightness then
                e.brightness = saved.brightness
                if propPtrs[i] then propPtrs[i].brightness[0] = e.brightness end
            end
            if saved.range then
                e.range = saved.range
                if propPtrs[i] then propPtrs[i].range[0] = e.range end
            end
            if saved.innerAngle then
                e.innerAngle = saved.innerAngle
                if propPtrs[i] then propPtrs[i].innerAngle[0] = e.innerAngle end
            end
            if saved.outerAngle then
                e.outerAngle = saved.outerAngle
                if propPtrs[i] then propPtrs[i].outerAngle[0] = e.outerAngle end
            end
            if saved.castShadows ~= nil then
                e.castShadows = saved.castShadows
                if propPtrs[i] then propPtrs[i].castShadows[0] = e.castShadows end
            end
            if saved.flareScale then
                e.flareScale = saved.flareScale
                if propPtrs[i] then propPtrs[i].flareScale[0] = e.flareScale end
            end
            if saved.color then
                e.color.r, e.color.g, e.color.b = saved.color.r, saved.color.g, saved.color.b
            end
            if saved.flareName then e.flareName = saved.flareName end
        end
    end
    
    -- Push all to live
    for i = 1, livePropCount do pushProp(i) end
    
    synced = true
    setStatus("Loaded saved config for " .. currentVehName, false)
end

-- Reset to jbeam defaults (re-read from vehicle)
local function resetToDefaults()
    syncFromLive()
    setStatus("Reset to jbeam defaults", false)
end

-- ImGui helpers
local function dragPos(label, ptr, speed, min, max, fmt)
    speed = speed or 0.005
    min = min or -5.0
    max = max or 5.0
    fmt = fmt or "%.3f"
    return im.DragFloat(label, ptr, speed, min, max, fmt)
end

local function dragRot(label, ptr, speed, min, max, fmt)
    speed = speed or 0.25
    min = min or -360.0
    max = max or 360.0
    fmt = fmt or "%.1f deg"
    return im.DragFloat(label, ptr, speed, min, max, fmt)
end

local function dragAngle(label, ptr, speed, min, max, fmt)
    speed = speed or 0.5
    min = min or 0.0
    max = max or 180.0
    fmt = fmt or "%.1f deg"
    return im.DragFloat(label, ptr, speed, min, max, fmt)
end

local function dragFloat(label, ptr, speed, min, max, fmt)
    speed = speed or 0.01
    min = min or 0.0
    max = max or 10.0
    fmt = fmt or "%.3f"
    return im.DragFloat(label, ptr, speed, min, max, fmt)
end

-- Main ImGui window
local function drawWindow()
    im.SetNextWindowSize(im.ImVec2(450, 700), im.Cond_FirstUseEver)
    if not im.Begin(wndName, windowOpen) then
        im.End()
        return
    end
    
    -- Auto-load config for current vehicle
    local veh = be:getPlayerVehicle(0)
    currentVehName = veh and getVehName(veh) or nil
    
    if currentVehName ~= lastUIVehName then
        lastUIVehName = currentVehName
        uiLoadedForVehicle = false
        synced = false
    end
    
    if currentVehName then
        im.TextColored(im.ImVec4(0.4, 1, 0.4, 1), "Vehicle: " .. currentVehName)
    else
        im.TextColored(im.ImVec4(1, 0.6, 0.2, 1), "No vehicle spawned")
    end
    
    if not veh then
        im.Text("Spawn a vehicle with lighting props to edit.")
        im.End()
        return
    end
    
    if not synced then 
        syncFromLive() 
        synced = true
    end
    
    im.Spacing()
    
    -- Toolbar
    if im.Checkbox("Show Light Props##showLightProps", showPropDebug) then
        dlog('D', 'lightingPropsEditor', 'Show light props debug: ' .. tostring(showPropDebug[0]))
    end
    im.SameLine()
    if im.SmallButton("Resync from Vehicle##resyncBtn") then syncFromLive() end
    im.SameLine()
    if im.SmallButton("Reset to Defaults##resetBtn") then resetToDefaults() end
    
    im.Separator()
    
    -- Save/Load buttons
    if im.Button("Save Config##saveBtn") then saveConfig() end
    im.SameLine()
    if im.Button("Load Config##loadBtn") then loadConfig() end
    im.SameLine()
    if im.Button("Save & Apply##saveApplyBtn") then 
        saveConfig()
        for i = 1, livePropCount do pushProp(i) end
        setStatus("Saved and applied", false)
    end
    
    im.Separator()
    
    if livePropCount == 0 then
        im.TextColored(im.ImVec4(1, 0.6, 0.2, 1), "No SPOTLIGHT/POINTLIGHT props found")
        im.TextWrapped("Add props to jbeam with mesh=SPOTLIGHT or POINTLIGHT and a func name.")
    else
        for i = 1, livePropCount do
            local e = propEdit[i]
            local p = propPtrs[i]
            local header = string.format("Prop %d: %s (%s)##%d", i, e.func, e.mesh, e.pid)
            
            if im.CollapsingHeader1(header, im.TreeNodeFlags_DefaultOpen) then
                local changed = false
                
                -- Position (local to vehicle)
                im.TextColored(im.ImVec4(0.6, 1, 0.6, 1), "Position (local)")
                if dragPos("Pos X##" .. e.pid, p.x) then e.pos.x = p.x[0]; changed = true end
                if dragPos("Pos Y##" .. e.pid, p.y) then e.pos.y = p.y[0]; changed = true end
                if dragPos("Pos Z##" .. e.pid, p.z) then e.pos.z = p.z[0]; changed = true end
                im.Separator()
                
                -- Rotation
                im.TextColored(im.ImVec4(0.6, 1, 0.6, 1), "Rotation (degrees)")
                if dragRot("Rot X##" .. e.pid, p.rx) then e.rot.x = p.rx[0]; changed = true end
                if dragRot("Rot Y##" .. e.pid, p.ry) then e.rot.y = p.ry[0]; changed = true end
                if dragRot("Rot Z##" .. e.pid, p.rz) then e.rot.z = p.rz[0]; changed = true end
                im.Separator()
                
                -- Light properties
                im.TextColored(im.ImVec4(0.6, 1, 0.6, 1), "Light Properties")
                if dragFloat("Brightness##" .. e.pid, p.brightness, 0.01, 0.0, 10.0, "%.2f") then
                    e.brightness = p.brightness[0]; changed = true
                end
                if dragFloat("Range##" .. e.pid, p.range, 0.01, 0.01, 20.0, "%.2f m") then
                    e.range = p.range[0]; changed = true
                end
                if e.mesh == "SPOTLIGHT" then
                    if dragAngle("Inner Angle##" .. e.pid, p.innerAngle, 0.5, 0.0, 180.0, "%.1f deg") then
                        e.innerAngle = p.innerAngle[0]; changed = true
                    end
                    if dragAngle("Outer Angle##" .. e.pid, p.outerAngle, 0.5, 0.0, 180.0, "%.1f deg") then
                        e.outerAngle = p.outerAngle[0]; changed = true
                    end
                end
                if im.Checkbox("Cast Shadows##" .. e.pid, p.castShadows) then
                    e.castShadows = p.castShadows[0]; changed = true
                end
                im.Separator()
                
                -- Color display
                im.TextColored(im.ImVec4(0.6, 1, 0.6, 1), "Color (controlled by Lighting App)")
                local col = im.ImVec4(e.color.r / 255, e.color.g / 255, e.color.b / 255, 1.0)
                im.ColorButton("ColorPreview##" .. e.pid, col, 0, im.ImVec2(60, 30))
                im.SameLine()
                im.Text(string.format("RGB(%d, %d, %d)", e.color.r, e.color.g, e.color.b))
                
                -- Info
                im.Separator()
                im.TextDisabled("Mesh: " .. e.mesh .. " | PID: " .. tostring(e.pid))
                if e.electric then
                    im.TextDisabled("Electric: " .. e.electric)
                end
                
                -- Show global position from prop object
                local propObj = veh:getProp(e.pid)
                if propObj then
                    local gt = propObj:getBaseTranslationGlobal()
                    local gr = propObj:getBaseRotationGlobal()
                    im.TextDisabled(string.format("Global: (%.3f, %.3f, %.3f)", gt.x, gt.y, gt.z))
                    im.TextDisabled(string.format("Global Rot: (%.1f, %.1f, %.1f) deg", gr.x * 180/math.pi, gr.y * 180/math.pi, gr.z * 180/math.pi))
                end
                
                if changed then pushProp(i) end
            end
        end
    end
    
    im.Separator()
    
    if statusIsError then
        im.TextColored(im.ImVec4(1, 0.35, 0.35, 1), statusMsg)
    else
        im.TextColored(im.ImVec4(0.4, 1, 0.4, 1), statusMsg)
    end
    
    im.Separator()
    im.TextDisabled("Values apply live via propObj:setBaseTranslation()/Rotation()")
    im.TextDisabled("Save writes per-vehicle JSON to settings/bbc_stereo/lighting_props/")
    im.TextDisabled("Press 'L' to toggle this window")
    
    im.End()
end

M.onUpdate = function(dtReal, dtSim, dtRaw)
    -- Draw light prop debug visualization
    if showPropDebug[0] then
        local veh = be:getPlayerVehicle(0)
        if veh then
            -- jbeam (x right, y back, z up) -> world: flip the forward vector
            local dirFront = veh:getDirectionVector()
            local dirUp = veh:getDirectionVectorUp()
            local vehRot = quatFromDir(-dirFront, dirUp)
            local vehPos = veh:getPosition() -- world position of the ref node
            local refPos = getRefJbeamPos(veh) -- ref node in jbeam coords
            
            for i, e in ipairs(propEdit) do
                local propObj = veh:getProp(e.pid)
                if propObj then
                    -- Base translation is in jbeam space, relative to the jbeam origin
                    local localPos = propObj:getBaseTranslationGlobal()
                    local pos = vehPos + vehRot * (vec3(localPos.x, localPos.y, localPos.z) - refPos)
                    
                    local rotLocal = propObj:getBaseRotationGlobal()
                    local q = vehRot * quatFromEuler(rotLocal.x, rotLocal.y, rotLocal.z)
                    
                    -- Draw prop position sphere
                    debugDrawer:drawSphere(pos, 0.04, ColorF(1, 1, 0, 1))
                    
                    -- Draw prop func name
                    debugDrawer:drawTextAdvanced(pos + vec3(0, 0, 0.1), e.func, ColorF(1, 1, 1, 1), true, false, ColorI(0, 0, 0, 192), false, false)
                    
                    -- For SPOTLIGHT, draw light direction cone
                    if e.mesh == "SPOTLIGHT" then
                        local axis = q * vec3(0, 1, 0)  -- light forward is local +Y
                        local right = q * vec3(1, 0, 0)
                        local up = q * vec3(0, 0, 1)
                        -- Clamp so a 200 m range doesn't draw giant lines
                        local len = math.min(e.range or 1.0, 1.5)
                        -- Angles are full cone angles; use half-angle for the edge
                        local innerHalf = math.rad((e.innerAngle or 45) * 0.5)
                        local outerHalf = math.rad((e.outerAngle or 60) * 0.5)
                        
                        debugDrawer:drawLine(pos, pos + axis * len, ColorF(1, 1, 0.5, 1))
                        
                        for k = 0, 11 do
                            local phi = k * math.pi / 6
                            local radial = right * math.cos(phi) + up * math.sin(phi)
                            local innerDir = axis * math.cos(innerHalf) + radial * math.sin(innerHalf)
                            local outerDir = axis * math.cos(outerHalf) + radial * math.sin(outerHalf)
                            debugDrawer:drawLine(pos, pos + innerDir * len, ColorF(1, 1, 0.5, 0.5))
                            debugDrawer:drawLine(pos, pos + outerDir * len, ColorF(1, 0.8, 0.3, 0.3))
                        end
                        
                        debugDrawer:drawSphere(pos + axis * len, 0.03, ColorF(1, 0.8, 0.2, 0.5))
                    end
                end
            end
        end
    end
    
    -- Draw the window exactly once per frame (drawing it twice caused the
    -- duplicated menu and the Dear ImGui "conflicting ID" warning)
    if windowOpen[0] then drawWindow() end
end

M.onLightingPropsData = onLightingPropsData

-- Auto-apply saved prop config on vehicle spawn/reset (like screenService calibration)
local function applySavedPropConfig(vehId)
    local veh = be:getObjectByID(vehId)
    if not veh then return end
    
    local vehName = getVehName(veh)
    if not vehName then return end
    
    local data = jsonReadFile(getConfigPath(vehName))
    if not data or not data.props then
        dlog('D', 'lightingPropsEditor', 'No saved prop config for ' .. vehName)
        return
    end
    
    dlog('I', 'lightingPropsEditor', 'Auto-applying saved prop config for ' .. vehName)
    
    -- Apply saved positions/rotations to each prop
    for funcName, saved in pairs(data.props) do
        local pid = saved.pid
        if pid then
            local propObj = veh:getProp(pid)
            if propObj then
                if saved.pos then
                    propObj:setBaseTranslation(vec3(saved.pos.x, saved.pos.y, saved.pos.z))
                end
                if saved.rot then
                    propObj:setBaseRotation(vec3(saved.rot.x, saved.rot.y, saved.rot.z))
                end
                dlog('D', 'lightingPropsEditor', string.format('Applied saved pos/rot to %s (pid=%d)', funcName, pid))
            end
        end
    end
end

local function onVehicleSpawned(vehId)
    applySavedPropConfig(vehId)
end

local function onVehicleResetted(vehId)
    applySavedPropConfig(vehId)
end

M.onVehicleSpawned = onVehicleSpawned
M.onVehicleResetted = onVehicleResetted

M.open = function()
    windowOpen[0] = true
    synced = false
end

M.close = function()
    windowOpen[0] = false
end

M.toggle = function()
    windowOpen[0] = not windowOpen[0]
    synced = false
end

return M