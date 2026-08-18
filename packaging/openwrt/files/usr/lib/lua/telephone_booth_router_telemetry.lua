local fs = require "nixio.fs"
local ubus = require "ubus"

local M = {}

local BATTERY_PATH = "/sys/class/power_supply/cw221X-bat"
local CHARGER_PATH = "/sys/class/power_supply/charger"

local function trim(value)
  if value == nil then
    return nil
  end
  return value:match("^%s*(.-)%s*$")
end

local function read_text(path)
  local file = io.open(path, "r")
  if file == nil then
    return nil
  end
  local value = file:read("*a")
  file:close()
  value = trim(value)
  if value == "" then
    return nil
  end
  return value
end

local function read_number(path, divisor)
  local value = tonumber(read_text(path))
  if value == nil then
    return nil
  end
  return value / (divisor or 1)
end

local function read_boolean(path)
  local value = read_number(path)
  if value == nil then
    return nil
  end
  return value ~= 0
end

local function selected_usb_type(value)
  if value == nil then
    return nil
  end
  return value:match("%[([^%]]+)%]") or value
end

local function mcu_status()
  local connected, connection = pcall(ubus.connect)
  if not connected or connection == nil then
    return {}
  end

  local called, status = pcall(function()
    return connection:call("mcu", "status", {})
  end)
  if connection.close ~= nil then
    pcall(function()
      connection:close()
    end)
  end
  if not called or type(status) ~= "table" then
    return {}
  end
  return status
end

local function battery_snapshot(mcu)
  return {
    present = read_boolean(BATTERY_PATH .. "/present"),
    chargePercent = read_number(BATTERY_PATH .. "/capacity"),
    temperatureCelsius = read_number(BATTERY_PATH .. "/temp", 10),
    voltageVolts = read_number(BATTERY_PATH .. "/voltage_now", 1000000),
    currentAmperes = read_number(BATTERY_PATH .. "/current_now", 1000000),
    health = read_text(BATTERY_PATH .. "/health"),
    technology = read_text(BATTERY_PATH .. "/technology"),
    cycleCount = read_number(BATTERY_PATH .. "/cycle_count"),
    chargeCount = tonumber(mcu.charge_cnt),
    abnormal = mcu.abnormal,
    abnormalType = tonumber(mcu.abnormal_type),
  }
end

local function charger_snapshot(mcu)
  return {
    present = read_boolean(CHARGER_PATH .. "/present"),
    online = read_boolean(CHARGER_PATH .. "/online"),
    status = read_text(CHARGER_PATH .. "/status"),
    usbType = selected_usb_type(read_text(CHARGER_PATH .. "/usb_type")),
    manufacturer = read_text(CHARGER_PATH .. "/manufacturer"),
    model = read_text(CHARGER_PATH .. "/model_name"),
    chargeType = read_text(CHARGER_PATH .. "/charge_type"),
    inputVoltageLimitVolts = read_number(CHARGER_PATH .. "/input_voltage_limit", 1000000),
    inputCurrentLimitAmperes = read_number(CHARGER_PATH .. "/input_current_limit", 1000000),
    constantChargeVoltageVolts = read_number(
      CHARGER_PATH .. "/constant_charge_voltage",
      1000000
    ),
    constantChargeCurrentMaxAmperes = read_number(
      CHARGER_PATH .. "/constant_charge_current_max",
      1000000
    ),
    fastCharge = mcu.fastcharge,
    chargingStatus = tonumber(mcu.charging_status),
  }
end

local function valid_thermal_reading(name, millidegrees)
  if name == nil or millidegrees == nil then
    return false
  end
  if millidegrees <= -40000 or millidegrees > 150000 then
    return false
  end
  return not (name == "zeroc" and millidegrees == 0)
end

local function thermal_zones()
  local zones = {}
  for path in fs.glob("/sys/class/thermal/thermal_zone*") do
    local name = read_text(path .. "/type")
    local millidegrees = read_number(path .. "/temp")
    if valid_thermal_reading(name, millidegrees) then
      zones[#zones + 1] = {
        name = name,
        zone = path:match("([^/]+)$"),
        temperatureCelsius = millidegrees / 1000,
      }
    end
  end
  table.sort(zones, function(left, right)
    if left.name == right.name then
      return left.zone < right.zone
    end
    return left.name < right.name
  end)
  return zones
end

function M.collect()
  local mcu = mcu_status()
  return {
    battery = battery_snapshot(mcu),
    charger = charger_snapshot(mcu),
    thermalZones = thermal_zones(),
  }
end

function M.escape_label(value)
  if value == nil then
    return nil
  end
  return tostring(value):gsub("\\", "\\\\"):gsub("\n", "\\n"):gsub('"', '\\"')
end

return M
