-- screenTuner - real-time imgui editor for the Screen Input Framework config
-- Lets you drag the reference plane and trigger boxes live in-game,
-- then write the values back to the jsonc config files.
--
-- Usage (in the in-game console):
--   extensions.load("screenTuner")
--   extensions.screenTuner.open()     -- or .toggle() / .close()

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
  dlog('I', "screenTuner", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

local imgui = ui_imgui

local windowOpen = imgui.BoolPtr(false)
local statusMsg = "Ready."
local statusIsError = false
local synced = false

-- Edit state (plain tables) + imgui pointers, synced from live screenService state
local planeEdit = {x = 0, y = 0, z = 0, rx = 0, ry = 0, rz = 0, width = 0.2, height = 0.2}
local planePtrs = nil
local planeKey = "0"

local boxEdit = {}
local boxPtrs = {}
local liveBoxCount = 0

local triggerEdit = {}
local triggerPtrs = {}
local liveTrigCount = 0

local function ensurePtrs()
    if not planePtrs then
        planePtrs = {
            x = imgui.FloatPtr(0), y = imgui.FloatPtr(0), z = imgui.FloatPtr(0),
            rx = imgui.FloatPtr(0), ry = imgui.FloatPtr(0), rz = imgui.FloatPtr(0),
            width = imgui.FloatPtr(0.2), height = imgui.FloatPtr(0.2),
        }
    end
end

local function setStatus(msg, isError)
    statusMsg = msg
    statusIsError = isError and true or false
end

-- Pull current live values from screenService into the edit tables
local function syncFromLive()
    if not screenService or not screenService.tunerGetState then
        setStatus("screenService not loaded - spawn the vehicle first", true)
        return
    end

    local planes, boxes, triggers = screenService.tunerGetState()
    ensurePtrs()

    planeKey = "0"
    local plane = planes and planes["0"]
    if not plane and planes and next(planes) then
        local k, v = next(planes)
        planeKey, plane = k, v
    end

    if plane then
        planeEdit.x, planeEdit.y, planeEdit.z = plane.pos.x, plane.pos.y, plane.pos.z
        planeEdit.rx, planeEdit.ry, planeEdit.rz = plane.rot.x or 0, plane.rot.y or 0, plane.rot.z or 0
        planeEdit.width = plane.width or 0.2
        planeEdit.height = plane.height or 0.2
        planePtrs.x[0], planePtrs.y[0], planePtrs.z[0] = planeEdit.x, planeEdit.y, planeEdit.z
        planePtrs.rx[0], planePtrs.ry[0], planePtrs.rz[0] = planeEdit.rx, planeEdit.ry, planeEdit.rz
        planePtrs.width[0], planePtrs.height[0] = planeEdit.width, planeEdit.height
    end

    boxEdit, boxPtrs = {}, {}
    liveBoxCount = boxes and #boxes or 0
    for i = 1, liveBoxCount do
        local box = boxes[i]
        boxEdit[i] = {
            x = box.pos.x, y = box.pos.y, z = box.pos.z,
            rx = box.rot and box.rot.x or 0, ry = box.rot and box.rot.y or 0, rz = box.rot and box.rot.z or 0,
            width = box.size and box.size.x or 0.2,
            tx = box.translateX or 0, ty = box.translateY or 0,
            id = box.id, screenId = box.screenId,
        }
        boxPtrs[i] = {
            x = imgui.FloatPtr(boxEdit[i].x), y = imgui.FloatPtr(boxEdit[i].y), z = imgui.FloatPtr(boxEdit[i].z),
            rx = imgui.FloatPtr(boxEdit[i].rx), ry = imgui.FloatPtr(boxEdit[i].ry), rz = imgui.FloatPtr(boxEdit[i].rz),
            width = imgui.FloatPtr(boxEdit[i].width),
            tx = imgui.FloatPtr(boxEdit[i].tx), ty = imgui.FloatPtr(boxEdit[i].ty),
        }
    end

    triggerEdit, triggerPtrs = {}, {}
    liveTrigCount = triggers and #triggers or 0
    for i = 1, liveTrigCount do
        local tg = triggers[i]
        triggerEdit[i] = {
            x = tg.pos.x, y = tg.pos.y, z = tg.pos.z,
            sx = tg.size and tg.size.x or 0.02,
            sy = tg.size and tg.size.y or 0.02,
            sz = tg.size and tg.size.z or 0.02,
            rx = tg.rot and tg.rot.x or 0, ry = tg.rot and tg.rot.y or 0, rz = tg.rot and tg.rot.z or 0,
            id = tg.id,
        }
        triggerPtrs[i] = {
            x = imgui.FloatPtr(triggerEdit[i].x), y = imgui.FloatPtr(triggerEdit[i].y), z = imgui.FloatPtr(triggerEdit[i].z),
            sx = imgui.FloatPtr(triggerEdit[i].sx), sy = imgui.FloatPtr(triggerEdit[i].sy), sz = imgui.FloatPtr(triggerEdit[i].sz),
            rx = imgui.FloatPtr(triggerEdit[i].rx), ry = imgui.FloatPtr(triggerEdit[i].ry), rz = imgui.FloatPtr(triggerEdit[i].rz),
        }
    end

    synced = true
    setStatus("Ready.", false)
end

local function pushPlane()
    if screenService and screenService.tunerSetRefPlane then
        screenService.tunerSetRefPlane(planeKey, planeEdit,
            {x = planeEdit.rx, y = planeEdit.ry, z = planeEdit.rz},
            {width = planeEdit.width, height = planeEdit.height})
    end
end

local function pushBox(i)
    if screenService and screenService.tunerSetBox then
        local e = boxEdit[i]
        screenService.tunerSetBox(i,
            {x = e.x, y = e.y, z = e.z},
            {x = e.rx, y = e.ry, z = e.rz},
            e.width, e.tx, e.ty)
    end
end

local function pushTrigger(i)
    if screenService and screenService.tunerSetTrigger then
        local e = triggerEdit[i]
        screenService.tunerSetTrigger(i,
            {x = e.x, y = e.y, z = e.z},
            {x = e.sx, y = e.sy, z = e.sz},
            {x = e.rx, y = e.ry, z = e.rz})
    end
end

-- Find the triggerBoxes config file for the current vehicle's model
local function findBoxesFile()
    local candidates = {}

    -- Prefer the BBC Stereo unpacked mod folder
    table.insert(candidates, "mods/unpacked/bbc_stereo_0.6/vehicles/common/BBC_Universal_Screen/")

    local veh = be:getPlayerVehicle(0)
    if veh then
        local ok, model = pcall(veh.getJBeamFilename, veh)
        if ok and model and model ~= "" then
            table.insert(candidates, "vehicles/" .. model .. "/")
        end
    end

    -- Fallback: the BBC Universal Screen folder
    table.insert(candidates, "vehicles/common/BBC_Universal_Screen/")

    for _, base in ipairs(candidates) do
        for _, pattern in ipairs({"*.jsonc", "*.json"}) do
            local files = FS:findFiles(base, pattern, 0, true, false)
            if files then
                for _, f in ipairs(files) do
                    -- never touch files inside zip mounts; only save to real (unpacked) files
                    if not f:find("%.zip") then
                        local content = readFile(f)
                        if content and content:match('"$configType"%s*:%s*"triggerBoxes"') then
                            f = (f:gsub("^/+", ""))
                            dlog("I", "screenTuner", "Found triggerBoxes file: " .. f)
                            return f
                        end
                    end
                end
            end
        end
        dlog("W", "screenTuner", "No triggerBoxes file under '" .. base .. "'")
    end
    return nil
end

local function reloadFromFiles()
    local path = findBoxesFile()
    if not path or not screenService or not screenService.loadBoxes then
        setStatus("Could not find triggerBoxes file for this vehicle", true)
        return
    end
    screenService.loadBoxes(path)
    syncFromLive()
    setStatus("Reloaded from " .. path, false)
end

local function saveAll()
    if not screenService or not screenService.tunerGetState then
        setStatus("screenService not loaded", true)
        return
    end

    local planes, boxes, triggers = screenService.tunerGetState()

    -- If the box list is empty (e.g. screenService was reloaded), load it from disk first
    if not boxes or not next(boxes) then
        local path = findBoxesFile()
        if path and screenService.loadBoxes then
            dlog("I", "screenTuner", "Boxes were empty - loading from " .. tostring(path))
            screenService.loadBoxes(path)
            planes, boxes, triggers = screenService.tunerGetState()
        end
    end

    local boxesPath = findBoxesFile()
    if not boxesPath then
        setStatus("Could not find triggerBoxes file to save into", true)
        return
    end
    if not boxes or not next(boxes) then
        setStatus("No boxes loaded to save", true)
        return
    end

    local okCount = 0

    -- Save reference plane(s) next to the boxes file
    if planes and next(planes) then
        local planeCount = 0
        for _ in pairs(planes) do planeCount = planeCount + 1 end

        local data
        if planeCount == 1 and planes["0"] then
            local p = planes["0"]
            data = {
                ["$configType"] = "referencePlane",
                pos = {x = p.pos.x, y = p.pos.y, z = p.pos.z},
                rot = {x = p.rot.x or 0, y = p.rot.y or 0, z = p.rot.z or 0},
                width = p.width or 0.2,
                height = p.height or 0.2,
            }
        else
            local arr = {}
            for id, p in pairs(planes) do
                table.insert(arr, {
                    id = id,
                    pos = {x = p.pos.x, y = p.pos.y, z = p.pos.z},
                    rot = {x = p.rot.x or 0, y = p.rot.y or 0, z = p.rot.z or 0},
                    width = p.width or 0.2,
                    height = p.height or 0.2,
                })
            end
            data = {["$configType"] = "referencePlane", planes = arr}
        end

        local dir = boxesPath:match("^(.+)/[^/]+$")
        if dir and jsonWriteFile(dir .. "/referencePlane.jsonc", data, true) then
            okCount = okCount + 1
        end
    end

    -- Save trigger boxes
    local boxData = {["$configType"] = "triggerBoxes", boxes = {}}
    for _, box in ipairs(boxes) do
        table.insert(boxData.boxes, {
            id = box.id,
            screenId = box.screenId,
            pos = {x = box.pos.x, y = box.pos.y, z = box.pos.z},
            scale = box.size and box.size.x or 0.2,
            depth = box.size and box.size.y or 0.0005,
            rot = {x = box.rot and box.rot.x or 0, y = box.rot and box.rot.y or 0, z = box.rot and box.rot.z or 0},
            refPlane = box.refPlane,
            translateX = box.translateX or 0,
            translateY = box.translateY or 0,
        })
    end

    if jsonWriteFile(boxesPath, boxData, true) then
        okCount = okCount + 1
    end

    -- Save trigger volumes (physical buttons) to the real mod folder
    if triggers and next(triggers) then
        local trigData = {["$configType"] = "triggers", triggers = {}}
        for _, tg in ipairs(triggers) do
            table.insert(trigData.triggers, {
                id = tg.id,
                pos = {x = tg.pos.x, y = tg.pos.y, z = tg.pos.z},
                size = {x = tg.size and tg.size.x or 0.02, y = tg.size and tg.size.y or 0.02, z = tg.size and tg.size.z or 0.02},
                rot = {x = tg.rot and tg.rot.x or 0, y = tg.rot and tg.rot.y or 0, z = tg.rot and tg.rot.z or 0},
                refPlane = tg.refPlane,
            })
        end
        if jsonWriteFile("mods/unpacked/bbc_stereo_0.6/vehicles/common/BBC_Universal_Screen/triggers.jsonc", trigData, true) then
            okCount = okCount + 1
        end
    end

    if okCount >= 1 then
        setStatus("Saved " .. okCount .. " file(s) to disk", false)
    else
        setStatus("Failed writing config files", true)
    end

    -- Also save as per-vehicle calibration (settings/bbc_stereo/calibrations/<jbeam>.json)
    local veh = be:getPlayerVehicle(0)
    if veh and screenService and screenService.saveCalibrationForVehicle then
        local vehId = veh:getID()
        if screenService.saveCalibrationForVehicle(vehId) then
            dlog("I", "screenTuner", "Saved per-vehicle calibration for " .. tostring(vehId))
        else
            dlog("W", "screenTuner", "Failed to save per-vehicle calibration")
        end
    end
end

local function dragPos(label, ptr)
    return imgui.DragFloat(label, ptr, 0.005, -5.0, 5.0, "%.3f")
end

local function dragRot(label, ptr)
    return imgui.DragFloat(label, ptr, 0.25, -360.0, 360.0, "%.1f deg")
end

local function drawWindow()
    imgui.SetNextWindowSize(imgui.ImVec2(430, 680), imgui.Cond_FirstUseEver)
    if not imgui.Begin("Screen Tuner", windowOpen) then
        imgui.End()
        return
    end

    if not screenService or not screenService.tunerGetState then
        imgui.TextColored(imgui.ImVec4(1, 0.35, 0.35, 1), "screenService is not loaded.")
        imgui.Text("Spawn a vehicle with a screenInput controller.")
        imgui.End()
        return
    end

    if not synced then syncFromLive() end

    -- resync if the set of loaded boxes/triggers changed (vehicle reset etc.)
    local planes, boxes, triggers = screenService.tunerGetState()
    local count = boxes and #boxes or 0
    local tcount = triggers and #triggers or 0
    if count ~= liveBoxCount or tcount ~= liveTrigCount then syncFromLive() end

    if imgui.SmallButton(screenService.drawBoxes and "Hide debug boxes" or "Show debug boxes") then
        screenService.drawBoxes = not screenService.drawBoxes
    end
    imgui.SameLine()
    if imgui.SmallButton("Reload from files") then reloadFromFiles() end
    imgui.SameLine()
    if imgui.SmallButton("Resync") then syncFromLive() end

    imgui.Separator()

    -- ===================== REFERENCE PLANE =====================
    if imgui.CollapsingHeader1("Reference Plane", imgui.TreeNodeFlags_DefaultOpen) then
        if planes and planes[planeKey] then
            local changed = false
            if dragPos("Pos X##plane", planePtrs.x) then planeEdit.x = planePtrs.x[0]; changed = true end
            if dragPos("Pos Y##plane", planePtrs.y) then planeEdit.y = planePtrs.y[0]; changed = true end
            if dragPos("Pos Z##plane", planePtrs.z) then planeEdit.z = planePtrs.z[0]; changed = true end
            imgui.Separator()
            if dragRot("Rot X##plane", planePtrs.rx) then planeEdit.rx = planePtrs.rx[0]; changed = true end
            if dragRot("Rot Y##plane", planePtrs.ry) then planeEdit.ry = planePtrs.ry[0]; changed = true end
            if dragRot("Rot Z##plane", planePtrs.rz) then planeEdit.rz = planePtrs.rz[0]; changed = true end
            imgui.Separator()
            if imgui.DragFloat("Width##plane", planePtrs.width, 0.005, 0.01, 2.0, "%.3f m") then
                planeEdit.width = planePtrs.width[0]; changed = true
            end
            if imgui.DragFloat("Height##plane", planePtrs.height, 0.005, 0.01, 2.0, "%.3f m") then
                planeEdit.height = planePtrs.height[0]; changed = true
            end
            if changed then pushPlane() end
        else
            imgui.TextColored(imgui.ImVec4(1, 0.6, 0.2, 1), "No reference plane loaded")
        end
    end

    -- ===================== TRIGGER BOXES =====================
    if imgui.CollapsingHeader1("Trigger Boxes", imgui.TreeNodeFlags_DefaultOpen) then
        if liveBoxCount == 0 then
            imgui.TextColored(imgui.ImVec4(1, 0.6, 0.2, 1), "No trigger boxes loaded")
        end
        for i = 1, liveBoxCount do
            local e = boxEdit[i]
            local p = boxPtrs[i]
            local header = string.format("Box %d%s (%s)", i, e.id and (": " .. e.id) or "", tostring(e.screenId))
            if imgui.CollapsingHeader1(header) then
                local changed = false
                if dragPos("Pos X##box" .. i, p.x) then e.x = p.x[0]; changed = true end
                if dragPos("Pos Y##box" .. i, p.y) then e.y = p.y[0]; changed = true end
                if dragPos("Pos Z##box" .. i, p.z) then e.z = p.z[0]; changed = true end
                if dragRot("Rot X##box" .. i, p.rx) then e.rx = p.rx[0]; changed = true end
                if dragRot("Rot Y##box" .. i, p.ry) then e.ry = p.ry[0]; changed = true end
                if dragRot("Rot Z##box" .. i, p.rz) then e.rz = p.rz[0]; changed = true end
                if imgui.DragFloat("Width##box" .. i, p.width, 0.005, 0.01, 2.0, "%.3f m") then
                    e.width = p.width[0]; changed = true
                end
                if imgui.DragFloat("Translate X##box" .. i, p.tx, 0.001, -0.1, 0.1, "%.3f") then
                    e.tx = p.tx[0]; changed = true
                end
                if imgui.DragFloat("Translate Y##box" .. i, p.ty, 0.001, -0.1, 0.1, "%.3f") then
                    e.ty = p.ty[0]; changed = true
                end
                if changed then pushBox(i) end
            end
        end
    end

    -- ===================== TRIGGER VOLUMES =====================
    if imgui.CollapsingHeader1("Trigger Volumes (physical buttons)") then
        if liveTrigCount == 0 then
            imgui.TextColored(imgui.ImVec4(1, 0.6, 0.2, 1), "No trigger volumes loaded")
        end
        for i = 1, liveTrigCount do
            local e = triggerEdit[i]
            local p = triggerPtrs[i]
            local header = string.format("Trigger %d%s", i, e.id and (": " .. e.id) or "")
            if imgui.CollapsingHeader1(header) then
                local changed = false
                if dragPos("Pos X##trg" .. i, p.x) then e.x = p.x[0]; changed = true end
                if dragPos("Pos Y##trg" .. i, p.y) then e.y = p.y[0]; changed = true end
                if dragPos("Pos Z##trg" .. i, p.z) then e.z = p.z[0]; changed = true end
                imgui.Separator()
                if imgui.DragFloat("Size X##trg" .. i, p.sx, 0.002, 0.002, 1.0, "%.3f m") then e.sx = p.sx[0]; changed = true end
                if imgui.DragFloat("Size Y##trg" .. i, p.sy, 0.002, 0.002, 1.0, "%.3f m") then e.sy = p.sy[0]; changed = true end
                if imgui.DragFloat("Size Z##trg" .. i, p.sz, 0.002, 0.002, 1.0, "%.3f m") then e.sz = p.sz[0]; changed = true end
                imgui.Separator()
                if dragRot("Rot X##trg" .. i, p.rx) then e.rx = p.rx[0]; changed = true end
                if dragRot("Rot Y##trg" .. i, p.ry) then e.ry = p.ry[0]; changed = true end
                if dragRot("Rot Z##trg" .. i, p.rz) then e.rz = p.rz[0]; changed = true end
                if changed then pushTrigger(i) end
            end
        end
    end
    imgui.Separator()

    if imgui.Button("Save all to config files") then saveAll() end
    if statusIsError then
        imgui.TextColored(imgui.ImVec4(1, 0.35, 0.35, 1), statusMsg)
    else
        imgui.TextColored(imgui.ImVec4(0.4, 1, 0.4, 1), statusMsg)
    end

    imgui.Separator()
    imgui.TextDisabled("Values apply live. Save writes the jsonc files")
    imgui.TextDisabled("(comments in the files are lost on save).")

    imgui.End()
end

M.onUpdate = function(dtReal, dtSim, dtRaw)
    if windowOpen[0] then drawWindow() end
end

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
