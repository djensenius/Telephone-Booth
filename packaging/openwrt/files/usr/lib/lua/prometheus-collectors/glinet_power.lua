local telemetry = require "telephone_booth_router_telemetry"

local function boolean_value(value)
  if value == nil then
    return nil
  end
  return value and 1 or 0
end

local function gauge(name, value)
  if value ~= nil then
    metric(name, "gauge", nil, value)
  end
end

local function info(name, labels)
  local populated = {}
  for key, value in pairs(labels) do
    if value ~= nil then
      populated[key] = telemetry.escape_label(value)
    end
  end
  if next(populated) ~= nil then
    metric(name, "gauge", populated, 1)
  end
end

local function scrape()
  local snapshot = telemetry.collect()
  local battery = snapshot.battery or {}
  local charger = snapshot.charger or {}

  gauge("glinet_battery_present", boolean_value(battery.present))
  gauge("glinet_battery_charge_percent", battery.chargePercent)
  gauge("glinet_battery_temperature_celsius", battery.temperatureCelsius)
  gauge("glinet_battery_voltage_volts", battery.voltageVolts)
  gauge("glinet_battery_current_amperes", battery.currentAmperes)
  gauge("glinet_battery_cycle_count", battery.cycleCount)
  gauge("glinet_battery_charge_count", battery.chargeCount)
  gauge("glinet_battery_abnormal", boolean_value(battery.abnormal))
  gauge("glinet_battery_abnormal_type", battery.abnormalType)
  info("glinet_battery_info", {
    health = battery.health,
    technology = battery.technology,
  })

  gauge("glinet_charger_present", boolean_value(charger.present))
  gauge("glinet_charger_online", boolean_value(charger.online))
  gauge("glinet_charger_fastcharge", boolean_value(charger.fastCharge))
  gauge("glinet_charger_charging_status", charger.chargingStatus)
  gauge("glinet_charger_input_voltage_limit_volts", charger.inputVoltageLimitVolts)
  gauge("glinet_charger_input_current_limit_amperes", charger.inputCurrentLimitAmperes)
  gauge("glinet_charger_constant_charge_voltage_volts", charger.constantChargeVoltageVolts)
  gauge(
    "glinet_charger_constant_charge_current_max_amperes",
    charger.constantChargeCurrentMaxAmperes
  )
  info("glinet_charger_info", {
    status = charger.status,
    usb_type = charger.usbType,
    manufacturer = charger.manufacturer,
    model = charger.model,
    charge_type = charger.chargeType,
  })

  local thermal = metric("glinet_thermal_temperature_celsius", "gauge")
  for _, zone in ipairs(snapshot.thermalZones or {}) do
    thermal({
      name = telemetry.escape_label(zone.name),
      zone = telemetry.escape_label(zone.zone),
    }, zone.temperatureCelsius)
  end
end

return { scrape = scrape }
