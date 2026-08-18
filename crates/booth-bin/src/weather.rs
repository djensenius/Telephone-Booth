//! Modeled outdoor-weather collection for environmental observability.

use serde::{Deserialize, Serialize};

#[cfg(feature = "weather")]
use std::fmt;
#[cfg(feature = "weather")]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(feature = "weather")]
use anyhow::{Context, Result, anyhow};
#[cfg(feature = "weather")]
use booth_metrics::{
    OutdoorWeatherSample, record_outdoor_weather_failure, record_outdoor_weather_gauges,
};
#[cfg(feature = "weather")]
use reqwest::Client;
#[cfg(feature = "weather")]
use tokio::task::JoinHandle;
#[cfg(feature = "weather")]
use tokio::time::MissedTickBehavior;
#[cfg(feature = "weather")]
use tracing::{debug, info, warn};

/// Default weather polling cadence.
pub const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 600;
/// Minimum supported weather polling cadence.
pub const MIN_POLL_INTERVAL_SECONDS: u64 = 60;
/// Maximum accepted per-request timeout.
pub const MAX_REQUEST_TIMEOUT_SECONDS: u64 = 60;

#[cfg(feature = "weather")]
const OPEN_METEO_URL: &str = "https://api.open-meteo.com/v1/forecast";
#[cfg(feature = "weather")]
const OPEN_METEO_SOURCE: &str = "open_meteo";
#[cfg(feature = "weather")]
const CURRENT_FIELDS: &str = "temperature_2m,relative_humidity_2m,apparent_temperature,precipitation,weather_code,cloud_cover,wind_speed_10m,shortwave_radiation";

/// Opt-in modeled outdoor-weather collection.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WeatherConfig {
    /// Enable the periodic provider request and Prometheus metrics.
    pub enabled: bool,
    /// Installation latitude in decimal degrees.
    pub latitude: Option<f64>,
    /// Installation longitude in decimal degrees.
    pub longitude: Option<f64>,
    /// Seconds between provider requests.
    pub poll_interval_seconds: u64,
    /// Per-request timeout in seconds.
    pub request_timeout_seconds: u64,
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            latitude: None,
            longitude: None,
            poll_interval_seconds: DEFAULT_POLL_INTERVAL_SECONDS,
            request_timeout_seconds: 15,
        }
    }
}

#[cfg(feature = "weather")]
#[derive(Serialize)]
struct OpenMeteoQuery {
    latitude: f64,
    longitude: f64,
    current: &'static str,
    temperature_unit: &'static str,
    wind_speed_unit: &'static str,
    precipitation_unit: &'static str,
    timeformat: &'static str,
    timezone: &'static str,
    forecast_days: u8,
}

#[cfg(feature = "weather")]
#[derive(Debug, Deserialize)]
struct OpenMeteoResponse {
    current: OpenMeteoCurrent,
}

#[cfg(feature = "weather")]
#[derive(Debug, Deserialize)]
struct OpenMeteoCurrent {
    time: i64,
    temperature_2m: f64,
    relative_humidity_2m: f64,
    apparent_temperature: f64,
    precipitation: f64,
    weather_code: u16,
    cloud_cover: f64,
    wind_speed_10m: f64,
    shortwave_radiation: f64,
}

#[cfg(feature = "weather")]
#[derive(Debug)]
struct WeatherFetchError {
    reason: &'static str,
    message: String,
}

#[cfg(feature = "weather")]
impl WeatherFetchError {
    fn new(reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }
}

#[cfg(feature = "weather")]
impl fmt::Display for WeatherFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[cfg(feature = "weather")]
impl std::error::Error for WeatherFetchError {}

/// Spawn the periodic weather collector.
#[cfg(feature = "weather")]
pub(crate) fn spawn_weather_collector(config: WeatherConfig) -> Result<JoinHandle<()>> {
    let latitude = config
        .latitude
        .ok_or_else(|| anyhow!("weather.latitude is required when weather is enabled"))?;
    let longitude = config
        .longitude
        .ok_or_else(|| anyhow!("weather.longitude is required when weather is enabled"))?;
    let client = Client::builder()
        .timeout(Duration::from_secs(config.request_timeout_seconds))
        .user_agent(format!("telephone-booth/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build outdoor weather HTTP client")?;

    Ok(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(config.poll_interval_seconds));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        info!(
            poll_interval_seconds = config.poll_interval_seconds,
            "outdoor weather collector started"
        );

        loop {
            ticker.tick().await;
            match fetch_current_weather(&client, latitude, longitude).await {
                Ok(sample) => {
                    record_outdoor_weather_gauges(&sample);
                    debug!(
                        observed_at_unix_seconds = sample.observed_at_unix_seconds,
                        temperature_celsius = sample.temperature_celsius,
                        condition = sample.condition,
                        "outdoor weather observation updated"
                    );
                }
                Err(error) => {
                    record_outdoor_weather_failure(OPEN_METEO_SOURCE, error.reason);
                    warn!(
                        reason = error.reason,
                        %error,
                        "outdoor weather refresh failed; retaining the last successful values"
                    );
                }
            }
        }
    }))
}

#[cfg(feature = "weather")]
async fn fetch_current_weather(
    client: &Client,
    latitude: f64,
    longitude: f64,
) -> Result<OutdoorWeatherSample, WeatherFetchError> {
    let response = client
        .get(OPEN_METEO_URL)
        .query(&OpenMeteoQuery {
            latitude,
            longitude,
            current: CURRENT_FIELDS,
            temperature_unit: "celsius",
            wind_speed_unit: "kmh",
            precipitation_unit: "mm",
            timeformat: "unixtime",
            timezone: "UTC",
            forecast_days: 1,
        })
        .send()
        .await
        .map_err(|error| WeatherFetchError::new("transport", error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(WeatherFetchError::new(
            "http_status",
            format!("Open-Meteo returned HTTP {status}"),
        ));
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| WeatherFetchError::new("transport", error.to_string()))?;
    let fetched_at_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| WeatherFetchError::new("clock", error.to_string()))?
        .as_secs();
    parse_open_meteo_response(&body, fetched_at_unix_seconds)
}

#[cfg(feature = "weather")]
fn parse_open_meteo_response(
    body: &[u8],
    fetched_at_unix_seconds: u64,
) -> Result<OutdoorWeatherSample, WeatherFetchError> {
    let response: OpenMeteoResponse = serde_json::from_slice(body)
        .map_err(|error| WeatherFetchError::new("decode", error.to_string()))?;
    let current = response.current;
    let observed_at_unix_seconds = u64::try_from(current.time)
        .map_err(|_| WeatherFetchError::new("invalid_data", "negative observation timestamp"))?;

    Ok(OutdoorWeatherSample {
        source: OPEN_METEO_SOURCE,
        observed_at_unix_seconds,
        fetched_at_unix_seconds,
        temperature_celsius: bounded("temperature_2m", current.temperature_2m, -100.0, 100.0)?,
        apparent_temperature_celsius: bounded(
            "apparent_temperature",
            current.apparent_temperature,
            -120.0,
            120.0,
        )?,
        relative_humidity_percent: bounded(
            "relative_humidity_2m",
            current.relative_humidity_2m,
            0.0,
            100.0,
        )?,
        cloud_cover_percent: bounded("cloud_cover", current.cloud_cover, 0.0, 100.0)?,
        precipitation_millimeters: bounded("precipitation", current.precipitation, 0.0, 1_000.0)?,
        wind_speed_kilometers_per_hour: bounded(
            "wind_speed_10m",
            current.wind_speed_10m,
            0.0,
            500.0,
        )?,
        shortwave_radiation_watts_per_square_meter: bounded(
            "shortwave_radiation",
            current.shortwave_radiation,
            0.0,
            2_000.0,
        )?,
        weather_code: current.weather_code,
        condition: condition_for_code(current.weather_code),
    })
}

#[cfg(feature = "weather")]
fn bounded(
    field: &'static str,
    value: f64,
    minimum: f64,
    maximum: f64,
) -> Result<f64, WeatherFetchError> {
    if value.is_finite() && (minimum..=maximum).contains(&value) {
        Ok(value)
    } else {
        Err(WeatherFetchError::new(
            "invalid_data",
            format!("{field} value {value} is outside {minimum}..={maximum}"),
        ))
    }
}

#[cfg(feature = "weather")]
fn condition_for_code(code: u16) -> &'static str {
    match code {
        0 => "clear_sky",
        1 => "mainly_clear",
        2 => "partly_cloudy",
        3 => "overcast",
        45 => "fog",
        48 => "rime_fog",
        51 | 53 | 55 => "drizzle",
        56 | 57 => "freezing_drizzle",
        61 | 63 | 65 => "rain",
        66 | 67 => "freezing_rain",
        71 | 73 | 75 => "snowfall",
        77 => "snow_grains",
        80..=82 => "rain_showers",
        85 | 86 => "snow_showers",
        95 => "thunderstorm",
        96 | 99 => "thunderstorm_with_hail",
        _ => "unknown",
    }
}

#[cfg(all(test, feature = "weather"))]
#[allow(clippy::expect_used, reason = "tests may panic on invalid fixtures")]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "current": {
        "time": 1787055300,
        "temperature_2m": 17.3,
        "relative_humidity_2m": 86,
        "apparent_temperature": 18.1,
        "precipitation": 0.0,
        "weather_code": 2,
        "cloud_cover": 37,
        "wind_speed_10m": 6.3,
        "shortwave_radiation": 234.0
      }
    }"#;

    #[test]
    fn parses_current_weather_into_stable_metrics() {
        let sample = parse_open_meteo_response(SAMPLE.as_bytes(), 1_787_055_312)
            .expect("fixture should parse");
        assert_eq!(sample.source, "open_meteo");
        assert_eq!(sample.observed_at_unix_seconds, 1_787_055_300);
        assert_eq!(sample.fetched_at_unix_seconds, 1_787_055_312);
        assert!((sample.temperature_celsius - 17.3).abs() < f64::EPSILON);
        assert_eq!(sample.condition, "partly_cloudy");
    }

    #[test]
    fn rejects_out_of_range_provider_data() {
        let invalid = SAMPLE.replace(
            "\"relative_humidity_2m\": 86",
            "\"relative_humidity_2m\": 101",
        );
        let error = parse_open_meteo_response(invalid.as_bytes(), 1_787_055_312)
            .expect_err("invalid humidity should fail");
        assert_eq!(error.reason, "invalid_data");
    }

    #[test]
    fn maps_wmo_codes_to_bounded_condition_labels() {
        assert_eq!(condition_for_code(0), "clear_sky");
        assert_eq!(condition_for_code(48), "rime_fog");
        assert_eq!(condition_for_code(65), "rain");
        assert_eq!(condition_for_code(86), "snow_showers");
        assert_eq!(condition_for_code(99), "thunderstorm_with_hail");
        assert_eq!(condition_for_code(42), "unknown");
    }
}
