//! Shelly smart-plug power-meter collection for environmental observability.

use serde::{Deserialize, Serialize};

#[cfg(feature = "shelly")]
use std::fmt;
#[cfg(feature = "shelly")]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(feature = "shelly")]
use anyhow::{Context, Result};
#[cfg(feature = "shelly")]
use booth_hal::ShellyDeviceStats;
#[cfg(feature = "shelly")]
use booth_metrics::{SystemSampler, record_shelly_device_failure, record_shelly_device_gauges};
#[cfg(feature = "shelly")]
use reqwest::{Client, Response};
#[cfg(feature = "shelly")]
use tokio::task::JoinHandle;
#[cfg(feature = "shelly")]
use tokio::time::MissedTickBehavior;
#[cfg(feature = "shelly")]
use tracing::{debug, info, warn};

/// Default Shelly polling cadence.
pub const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 15;
/// Minimum supported Shelly polling cadence.
pub const MIN_POLL_INTERVAL_SECONDS: u64 = 2;
/// Maximum accepted per-request timeout.
pub const MAX_REQUEST_TIMEOUT_SECONDS: u64 = 30;
/// Default IP for the Shelly Plug US Gen4 monitoring the Vivosun heat mat.
pub const DEFAULT_ENDPOINT: &str = "192.168.8.220";
/// Default bounded metric/snapshot label for the Vivosun heat mat plug.
pub const DEFAULT_DEVICE: &str = "vivosun_heat_mat";

#[cfg(feature = "shelly")]
const MAX_RESPONSE_BYTES: usize = 16 * 1024;
#[cfg(feature = "shelly")]
const MAX_RESPONSE_BYTES_U64: u64 = 16 * 1024;

/// Opt-in Shelly smart-plug power-meter collection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ShellyConfig {
    /// Enable the periodic Shelly HTTP/RPC request and Prometheus metrics.
    pub enabled: bool,
    /// Bounded local label for this meter, used in Prometheus labels and
    /// system snapshots.
    pub device: String,
    /// Device host/IP or base URL. A bare host is treated as `http://<host>`.
    pub endpoint: String,
    /// Shelly switch component id. Plugs generally use `0`.
    pub switch_id: u8,
    /// Seconds between device requests.
    pub poll_interval_seconds: u64,
    /// Per-request timeout in seconds.
    pub request_timeout_seconds: u64,
}

impl Default for ShellyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            device: DEFAULT_DEVICE.to_string(),
            endpoint: DEFAULT_ENDPOINT.to_string(),
            switch_id: 0,
            poll_interval_seconds: DEFAULT_POLL_INTERVAL_SECONDS,
            request_timeout_seconds: 5,
        }
    }
}

#[cfg(feature = "shelly")]
#[derive(Debug, Deserialize)]
struct ShellySwitchStatus {
    output: Option<bool>,
    apower: Option<f64>,
    voltage: Option<f64>,
    freq: Option<f64>,
    current: Option<f64>,
    aenergy: Option<ShellyEnergy>,
    temperature: Option<ShellyTemperature>,
}

#[cfg(feature = "shelly")]
#[derive(Debug, Deserialize)]
struct ShellyEnergy {
    total: Option<f64>,
}

#[cfg(feature = "shelly")]
#[derive(Debug, Deserialize)]
struct ShellyTemperature {
    #[serde(rename = "tC")]
    t_c: Option<f64>,
}

#[cfg(feature = "shelly")]
#[derive(Debug)]
struct ShellyFetchError {
    reason: &'static str,
    message: String,
}

#[cfg(feature = "shelly")]
impl ShellyFetchError {
    fn new(reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }
}

#[cfg(feature = "shelly")]
impl fmt::Display for ShellyFetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[cfg(feature = "shelly")]
impl std::error::Error for ShellyFetchError {}

/// Spawn the periodic Shelly collector.
#[cfg(feature = "shelly")]
pub(crate) fn spawn_shelly_collector(
    config: ShellyConfig,
    sampler: SystemSampler,
) -> Result<JoinHandle<()>> {
    let endpoint = normalize_endpoint(&config.endpoint);
    let client = Client::builder()
        .timeout(Duration::from_secs(config.request_timeout_seconds))
        .user_agent(format!("telephone-booth/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("build Shelly HTTP client")?;

    Ok(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(config.poll_interval_seconds));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        info!(
            device = %config.device,
            endpoint = %endpoint,
            poll_interval_seconds = config.poll_interval_seconds,
            "Shelly power collector started"
        );

        loop {
            ticker.tick().await;
            match fetch_shelly_status(&client, &endpoint, config.switch_id, &config.device).await {
                Ok(sample) => {
                    record_shelly_device_gauges(&sample);
                    sampler.record_shelly_device(sample.clone());
                    debug!(
                        device = %sample.device,
                        power_watts = sample.power_watts,
                        current_amperes = sample.current_amperes,
                        output_on = sample.output_on,
                        "Shelly power reading updated"
                    );
                }
                Err(error) => {
                    record_shelly_device_failure(&config.device, error.reason);
                    warn!(
                        device = %config.device,
                        endpoint = %endpoint,
                        reason = error.reason,
                        %error,
                        "Shelly power refresh failed; retaining the last successful values"
                    );
                }
            }
        }
    }))
}

#[cfg(feature = "shelly")]
async fn fetch_shelly_status(
    client: &Client,
    endpoint: &str,
    switch_id: u8,
    device: &str,
) -> Result<ShellyDeviceStats, ShellyFetchError> {
    let url = format!("{endpoint}/rpc/Switch.GetStatus?id={switch_id}");
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|error| ShellyFetchError::new("transport", error.without_url().to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(ShellyFetchError::new(
            "http",
            format!("Shelly returned HTTP {status}"),
        ));
    }
    let body = read_response_body(response).await?;
    let parsed: ShellySwitchStatus = serde_json::from_slice(&body)
        .map_err(|error| ShellyFetchError::new("json", error.to_string()))?;
    let fetched_at_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ShellyFetchError::new("clock", error.to_string()))?
        .as_secs();

    Ok(ShellyDeviceStats {
        device: device.to_string(),
        endpoint: Some(endpoint.to_string()),
        output_on: parsed.output,
        power_watts: parsed.apower,
        voltage_volts: parsed.voltage,
        frequency_hertz: parsed.freq,
        current_amperes: parsed.current,
        energy_watt_hours: parsed.aenergy.and_then(|energy| energy.total),
        temperature_celsius: parsed.temperature.and_then(|temperature| temperature.t_c),
        fetched_at_unix_seconds: Some(fetched_at_unix_seconds),
    })
}

#[cfg(feature = "shelly")]
async fn read_response_body(mut response: Response) -> Result<Vec<u8>, ShellyFetchError> {
    if let Some(content_length) = response.content_length()
        && content_length > MAX_RESPONSE_BYTES_U64
    {
        return Err(ShellyFetchError::new(
            "response_too_large",
            format!(
                "Shelly declared a {content_length}-byte response, exceeding the \
                 {MAX_RESPONSE_BYTES}-byte limit"
            ),
        ));
    }

    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default()
        .min(MAX_RESPONSE_BYTES);
    let mut body = Vec::with_capacity(capacity);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| ShellyFetchError::new("transport", error.without_url().to_string()))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(ShellyFetchError::new(
                "response_too_large",
                format!("Shelly response exceeded the {MAX_RESPONSE_BYTES}-byte limit"),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(feature = "shelly")]
fn normalize_endpoint(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_points_at_known_heat_mat_shelly() {
        let config = ShellyConfig::default();
        assert_eq!(config.endpoint, "192.168.8.220");
        assert_eq!(config.device, "vivosun_heat_mat");
        assert!(!config.enabled);
    }

    #[cfg(feature = "shelly")]
    #[test]
    fn normalize_endpoint_adds_http_for_bare_host() {
        assert_eq!(normalize_endpoint("192.168.8.220/"), "http://192.168.8.220");
        assert_eq!(
            normalize_endpoint("http://192.168.8.220/"),
            "http://192.168.8.220"
        );
    }
}
