-- lnequMaps - pushes a live local minimap slice (stock BeamNG road graph +
-- waypoint route) into the LNEQU infotainment screen webview.
-- Adapted from the WIP CarPlay mod's carplayBridge (proven approach).

local M = {}

local debugLogging = false

local function dlog(level, tag, msg)
  if debugLogging then
    log(level, tag, msg)
  end
end

function M.setDebugLogging(enabled)
  debugLogging = enabled and true or false
  dlog('I', "lnequMaps", "Debug logging " .. (debugLogging and "ENABLED" or "disabled"))
end

local UPDATE_HZ = 5
local RADIUS = 10000  -- 10km radius for full map data
local acc = 0

local function clamp10(v) return math.floor(v * 10) / 10 end

local function vehHeading(veh)
    local d = veh:getDirectionVector()
    if not d then return 0 end
    return math.atan2(d.x, d.y)
end

local function vehSpeedKmh(veh)
    local v = veh:getVelocity()
    if v then return v:length() * 3.6 end
    return 0
end

-- Reads BeamNG's active waypoint route and returns polyline/dest/nav info
-- relative to the vehicle position
local function buildNav(veh)
    local out = { route = {}, nav = { active = false } }
    if not (core_groundMarkers and core_groundMarkers.routePlanner) then return out end
    local path = core_groundMarkers.routePlanner.path
    if not path or not path[1] or not path[1].pos then return out end

    local pp = veh:getPosition()
    local cx, cy = pp.x, pp.y
    local n = #path
    local destDist = path[1].distToTarget or 0
    local clipRange = RADIUS * 1.25  -- square half-width/height

    local stride = math.max(1, math.floor(n / 200))
    local pts = {}
    for i = 1, n, stride do
        local p = path[i].pos
        if p then
            local dx, dy = p.x - cx, p.y - cy
            -- Square bounding box instead of circular
            if math.abs(dx) <= clipRange and math.abs(dy) <= clipRange then pts[#pts + 1] = { clamp10(dx), clamp10(dy) } end
        end
    end
    out.route = pts

    local dp = path[n].pos
    if dp then
        local ddx, ddy = dp.x - cx, dp.y - cy
        if math.abs(ddx) <= clipRange and math.abs(ddy) <= clipRange then out.dest = { clamp10(ddx), clamp10(ddy) } end
    end

    -- next turn: where the route bearing first deviates ~28deg from the
    -- vehicle's current direction of travel
    local function bearingAhead(i, dist)
        local d0 = path[i].distToTarget or 0
        for j = i + 1, n do
            if (d0 - (path[j].distToTarget or 0)) >= dist then
                local a, b = path[i].pos, path[j].pos
                return math.atan2(b.x - a.x, b.y - a.y)
            end
        end
        return nil
    end

    local speedKmh = vehSpeedKmh(veh)
    local etaMin
    if speedKmh and speedKmh > 1 then etaMin = math.floor((destDist / 1000) / speedKmh * 60 + 0.5) end

    local turnDir, turnDist
    local baseB = bearingAhead(1, 12)
    if baseB then
        for i = 2, n - 1 do
            local distFromVeh = destDist - (path[i].distToTarget or 0)
            if distFromVeh > 600 then break end
            local b = bearingAhead(i, 12)
            if b then
                local diff = b - baseB
                while diff > math.pi do diff = diff - 2 * math.pi end
                while diff < -math.pi do diff = diff + 2 * math.pi end
                if math.abs(diff) > 0.5 then
                    turnDir = (diff > 0) and "right" or "left"
                    turnDist = destDist - (path[i].distToTarget or 0)
                    break
                end
            end
        end
    end

    out.nav = {
        active = true,
        dist = math.floor(destDist + 0.5),
        etaMin = etaMin,
        turn = turnDir or ((destDist < 30) and "arrive" or "straight"),
        turnDist = turnDist and math.floor(turnDist + 0.5) or nil,
    }
    return out
end

local function tickMap(veh)
    local m = map.getMap()
    if not m or not m.nodes then return end
    local pp = veh:getPosition()
    local cx, cy = pp.x, pp.y
    local halfRange = RADIUS  -- square half-width/height

    local nameToIdx = {}
    local nodes = {}
    for nid, node in pairs(m.nodes) do
        if node.pos then
            local dx = node.pos.x - cx
            local dy = node.pos.y - cy
            -- Square bounding box instead of circular radius
            if math.abs(dx) <= halfRange and math.abs(dy) <= halfRange then
                nameToIdx[nid] = #nodes + 1
                -- Include radius (road half-width) for proper road rendering
                nodes[#nodes + 1] = { math.floor(dx * 10) / 10, math.floor(dy * 10) / 10, node.radius or 3 }
            end
        end
    end

    local seen = {}
    local edges = {}
    for nid, node in pairs(m.nodes) do
        local a = nameToIdx[nid]
        if a and node.links then
            for tgt, _ in pairs(node.links) do
                local b = nameToIdx[tgt]
                if b and a ~= b then
                    local key = (a < b) and (a .. "_" .. b) or (b .. "_" .. a)
                    if not seen[key] then
                        seen[key] = true
                        edges[#edges + 1] = { a, b }
                    end
                end
            end
        end
    end

    local navData = buildNav(veh)
    local payload = {
        n = nodes,
        e = edges,
        h = math.floor(vehHeading(veh) * 1000) / 1000,
        s = math.floor(vehSpeedKmh(veh) * 10) / 10,
        r = RADIUS,
        route = navData.route,
        dest = navData.dest,
        nav = navData.nav,
    }

    local ok, js = pcall(jsonEncode, payload)
    if not ok then return end
    -- Use %q to properly escape for Lua string literal
    local escapedJs = string.format('%q', js)
    veh:queueLuaCommand('if screenInput and screenInput.pushToScreens then screenInput.pushToScreens("lnequMapData", jsonDecode(' .. escapedJs .. ')) end')
end

function M.onUpdate(dtReal)
    acc = acc + (dtReal or 0)
    if acc < 1 / UPDATE_HZ then return end
    acc = 0
    local veh = be and be.getPlayerVehicle and be:getPlayerVehicle(0) or nil
    if not veh then return end
    pcall(tickMap, veh)
end

M.onExtensionLoaded = function()
    dlog("I", "lnequMaps", "loaded")
end

return M