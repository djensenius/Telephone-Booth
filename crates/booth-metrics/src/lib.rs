//! Host-vitals sampler and Prometheus metrics registry for the Telephone
//! Booth phone client.
//!
//! This crate produces a [`booth_hal::SystemSnapshot`] at a configurable
//! cadence by reading from [`sysinfo`] (cross-platform CPU / memory / disk
//! / network / uptime) and, on Linux, the Pi's `thermal_zone0` sysfs node.
//! It also owns the in-process [`metrics`] registry: each sample updates a
//! handful of gauges (`booth_cpu_temperature_celsius`, …) and each
//! observed [`TelemetryEvent`] updates the appropriate counter or
//! histogram (`booth_calls_total{outcome=…}`, `booth_digits_dialed_total{digit=…}`,
//! `booth_recording_duration_seconds`, …).
//!
//! In this PR the crate is freestanding: nothing in `booth-bin` calls
//! into it yet. The follow-up PR (`tb-runtime`) wires
//! [`spawn_system_sampler`] and [`spawn_telemetry_consumer`] into the
//! runtime task set and adds the `/v1/system` debug route. A later PR
//! (`tb-metrics-and-docs`) layers the Prometheus text exposition onto
//! `booth-debug` and adds the vmagent sidecar.
//!
//! # Cross-platform sampling
//!
//! All cross-platform stats come from [`sysinfo`] so the simulator on
//! macOS sees populated values too. The Pi-specific reads are gated with
//! `#[cfg(target_os = "linux")]` and silently return `None` on non-Linux
//! hosts. `vcgencmd get_throttled` is not invoked yet; the
//! [`booth_hal::ThrottlingFlags`] field stays `None` for now and lands in
//! a follow-up PR alongside the systemd `video` group documentation.

#![warn(missing_docs)]

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use booth_hal::{
    AudioChannel, AudioDeviceStats, CallOutcome, CpuStats, DiskStats, MemoryStats, NetworkStats,
    ProcessStats, SystemSnapshot, TelemetryEvent,
};
use booth_telemetry::TelemetryBus;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

#[cfg(feature = "system")]
use sysinfo::{Disks, MemoryRefreshKind, Networks, ProcessRefreshKind, RefreshKind, System};

/// Default sampling cadence used when no explicit interval is supplied.
pub const DEFAULT_SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// Errors that can occur while installing the metrics registry.
#[derive(Debug, Error)]
pub enum MetricsError {
    /// The global metrics recorder has already been installed by another
    /// caller. The previous handle remains authoritative.
    #[error("metrics recorder already installed for this process")]
    AlreadyInstalled,
    /// Building the Prometheus exporter failed.
    #[error("failed to build prometheus recorder: {0}")]
    Build(String),
}

/// Owned handle to the booth's installed Prometheus registry.
///
/// In this PR the handle is held by the runtime but its `render` output
/// is intentionally not exposed over HTTP — that arrives in
/// `tb-metrics-and-docs`. Tests use [`MetricsHandle::render`] to assert
/// the expected series exist.
#[derive(Clone)]
pub struct MetricsHandle {
    inner: Arc<MetricsHandleInner>,
}

struct MetricsHandleInner {
    handle: PrometheusHandle,
    booth_id: String,
}

impl MetricsHandle {
    /// Booth identifier embedded as the `booth_id` label on every series.
    #[must_use]
    pub fn booth_id(&self) -> &str {
        &self.inner.booth_id
    }

    /// Render the current Prometheus text exposition.
    ///
    /// Used by tests in this PR. The follow-up `tb-metrics-and-docs` PR
    /// exposes the same text over `GET /metrics` on `booth-debug`'s
    /// loopback listener.
    #[must_use]
    pub fn render(&self) -> String {
        self.inner.handle.render()
    }
}

/// Tracks installer state so tests can install at most once per process.
static INSTALLED: OnceLock<MetricsHandle> = OnceLock::new();
static INSTALL_LOCK: Mutex<()> = Mutex::new(());

/// Install the booth metrics registry as the global [`metrics`] recorder.
///
/// Returns the existing handle if a prior call has already installed the
/// recorder; this lets tests share one global recorder while production
/// code installs it once at startup.
pub fn install_registry(booth_id: impl Into<String>) -> Result<MetricsHandle, MetricsError> {
    let booth_id = booth_id.into();
    if let Some(existing) = INSTALLED.get() {
        if existing.booth_id() != booth_id {
            warn!(
                existing = existing.booth_id(),
                requested = %booth_id,
                "booth-metrics registry already installed with a different booth_id; keeping the existing handle"
            );
        }
        return Ok(existing.clone());
    }

    // Serialize installation. `install_recorder` mutates global state and
    // will fail the second time it's called, so we double-check the
    // OnceLock under this lock to ensure exactly one caller wins the race.
    let _guard = INSTALL_LOCK.lock();
    if let Some(existing) = INSTALLED.get() {
        if existing.booth_id() != booth_id {
            warn!(
                existing = existing.booth_id(),
                requested = %booth_id,
                "booth-metrics registry already installed with a different booth_id; keeping the existing handle"
            );
        }
        return Ok(existing.clone());
    }

    let recorder_handle = PrometheusBuilder::new()
        .add_global_label("booth_id", &booth_id)
        .install_recorder()
        .map_err(|err| MetricsError::Build(err.to_string()))?;

    let handle = MetricsHandle {
        inner: Arc::new(MetricsHandleInner {
            handle: recorder_handle,
            booth_id,
        }),
    };

    INSTALLED
        .set(handle.clone())
        .map_err(|_| MetricsError::AlreadyInstalled)?;
    Ok(handle)
}

// ---------------------------------------------------------------------------
// System sampler
// ---------------------------------------------------------------------------

/// Configuration for the periodic system sampler.
#[derive(Debug, Clone, Copy)]
pub struct SamplerConfig {
    /// How often to take a sample. Defaults to [`DEFAULT_SAMPLE_INTERVAL`].
    pub interval: Duration,
}

impl Default for SamplerConfig {
    fn default() -> Self {
        Self {
            interval: DEFAULT_SAMPLE_INTERVAL,
        }
    }
}

/// Builds [`SystemSnapshot`]s on demand and updates the booth's gauges.
///
/// The sampler subscribes to [`TelemetryEvent::AudioDeviceChange`] events
/// so that `SystemSnapshot::audio` always reports the most recently
/// observed device names without having to crack open the audio adapter.
pub struct SystemSampler {
    inner: Arc<SystemSamplerInner>,
}

impl Clone for SystemSampler {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct SystemSamplerInner {
    #[cfg(feature = "system")]
    sysinfo: Mutex<SysinfoState>,
    audio: Mutex<AudioDeviceStats>,
}

#[cfg(feature = "system")]
struct SysinfoState {
    system: System,
    disks: Disks,
    networks: Networks,
}

impl SystemSampler {
    /// Create a fresh sampler. The first sample call performs an initial
    /// `sysinfo` refresh.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(SystemSamplerInner {
                #[cfg(feature = "system")]
                sysinfo: Mutex::new(SysinfoState {
                    system: System::new_with_specifics(
                        RefreshKind::new()
                            .with_cpu(sysinfo::CpuRefreshKind::everything())
                            .with_memory(MemoryRefreshKind::everything())
                            .with_processes(ProcessRefreshKind::everything()),
                    ),
                    disks: Disks::new_with_refreshed_list(),
                    networks: Networks::new_with_refreshed_list(),
                }),
                audio: Mutex::new(AudioDeviceStats::default()),
            }),
        }
    }

    /// Record the latest audio device name observed on the bus.
    ///
    /// Public so `booth-bin` can wire bus subscribers up directly without
    /// having to expose the sampler internals.
    pub fn record_audio_device(&self, channel: AudioChannel, name: String) {
        let mut audio = self.inner.audio.lock();
        match channel {
            AudioChannel::Input => audio.input_device = Some(name),
            AudioChannel::Output => audio.output_device = Some(name),
        }
    }

    /// Take one snapshot, refreshing the underlying `sysinfo` state. Cheap
    /// enough to call on a 5 s cadence — `sysinfo` reuses its allocations.
    #[must_use]
    pub fn sample_once(&self) -> SystemSnapshot {
        let mut snapshot = SystemSnapshot {
            audio: Some(self.inner.audio.lock().clone()),
            temperature_celsius: read_cpu_temp_celsius(),
            ..SystemSnapshot::default()
        };
        #[cfg(feature = "system")]
        {
            let mut state = self.inner.sysinfo.lock();
            state.system.refresh_cpu_all();
            state
                .system
                .refresh_memory_specifics(MemoryRefreshKind::everything());
            state.disks.refresh();
            state.networks.refresh();

            snapshot.cpu = Some(cpu_stats(&state.system));
            snapshot.memory = Some(memory_stats(&state.system));
            snapshot.disks = disk_stats(&state.disks);
            snapshot.networks = network_stats(&state.networks);
            snapshot.uptime_seconds = Some(System::uptime());
            snapshot.process = process_stats(&mut state.system);
        }
        snapshot
    }
}

impl Default for SystemSampler {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Gauge updates
// ---------------------------------------------------------------------------

/// Publish gauges for a freshly captured snapshot.
///
/// This is split out from [`SystemSampler::sample_once`] so callers can
/// take a snapshot, log it / publish it on the bus, and write metrics in
/// whatever order they prefer. Metric writes are no-ops if no recorder
/// is installed.
pub fn record_snapshot_gauges(snapshot: &SystemSnapshot) {
    if let Some(temp) = snapshot.temperature_celsius {
        metrics::gauge!("booth_cpu_temperature_celsius").set(f64::from(temp));
    }
    if let Some(cpu) = &snapshot.cpu {
        metrics::gauge!("booth_cpu_usage_ratio").set(f64::from(cpu.usage_ratio));
        if let Some(la) = cpu.load_avg_1m {
            metrics::gauge!("booth_load_average", "window" => "1m").set(f64::from(la));
        }
        if let Some(la) = cpu.load_avg_5m {
            metrics::gauge!("booth_load_average", "window" => "5m").set(f64::from(la));
        }
        if let Some(la) = cpu.load_avg_15m {
            metrics::gauge!("booth_load_average", "window" => "15m").set(f64::from(la));
        }
    }
    if let Some(mem) = &snapshot.memory {
        // Cast loses precision above 2^53 bytes but the Prometheus wire
        // format only carries f64, and 8 PB of RAM is well outside scope.
        #[allow(clippy::cast_precision_loss)]
        {
            metrics::gauge!("booth_memory_used_bytes").set(mem.used_bytes as f64);
            metrics::gauge!("booth_memory_total_bytes").set(mem.total_bytes as f64);
        }
    }
    for disk in &snapshot.disks {
        let mountpoint = disk.mount_point.clone();
        #[allow(clippy::cast_precision_loss)]
        {
            metrics::gauge!(
                "booth_disk_used_bytes",
                "mountpoint" => mountpoint.clone(),
            )
            .set((disk.total_bytes.saturating_sub(disk.available_bytes)) as f64);
            metrics::gauge!(
                "booth_disk_total_bytes",
                "mountpoint" => mountpoint,
            )
            .set(disk.total_bytes as f64);
        }
    }
    for net in &snapshot.networks {
        let iface = net.interface.clone();
        #[allow(clippy::cast_precision_loss)]
        {
            metrics::counter!(
                "booth_network_receive_bytes_total",
                "iface" => iface.clone(),
            )
            .absolute(net.receive_bytes_total);
            metrics::counter!(
                "booth_network_transmit_bytes_total",
                "iface" => iface,
            )
            .absolute(net.transmit_bytes_total);
        }
    }
    if let Some(uptime) = snapshot.uptime_seconds {
        #[allow(clippy::cast_precision_loss)]
        metrics::gauge!("booth_uptime_seconds").set(uptime as f64);
    }
}

// ---------------------------------------------------------------------------
// Telemetry consumer
// ---------------------------------------------------------------------------

/// Update counters and histograms in response to each telemetry event.
///
/// Public so `booth-bin` and tests can drive this from any source of
/// [`TelemetryEvent`]s — not just the [`TelemetryBus`].
pub fn record_telemetry_event(event: &TelemetryEvent) {
    match event {
        TelemetryEvent::DigitDialed { digit, .. } => {
            metrics::counter!(
                "booth_digits_dialed_total",
                "digit" => digit.to_string(),
            )
            .increment(1);
        }
        TelemetryEvent::StateTransition { from, to, .. } => {
            metrics::counter!(
                "booth_state_transitions_total",
                "from" => from.clone(),
                "to" => to.clone(),
            )
            .increment(1);
        }
        TelemetryEvent::AudioLevel(level) => {
            let channel = match level.channel {
                AudioChannel::Input => "input",
                AudioChannel::Output => "output",
            };
            metrics::gauge!(
                "booth_audio_peak_amplitude",
                "channel" => channel,
            )
            .set(f64::from(level.peak));
            metrics::gauge!(
                "booth_audio_rms_amplitude",
                "channel" => channel,
            )
            .set(f64::from(level.rms));
        }
        TelemetryEvent::OperatorRequest { .. } => {
            // OperatorRequest carries no status, so the counter is bumped
            // on the matching OperatorResponse instead.
        }
        TelemetryEvent::OperatorResponse {
            status,
            duration_ms,
            ..
        } => {
            let class = format!("{}xx", status / 100);
            metrics::counter!(
                "booth_operator_requests_total",
                "status_class" => class,
            )
            .increment(1);
            metrics::histogram!("booth_operator_request_duration_seconds")
                .record(secs_from_millis(*duration_ms));
        }
        TelemetryEvent::Error { source, .. } => {
            metrics::counter!(
                "booth_errors_total",
                "source" => source.clone(),
            )
            .increment(1);
        }
        TelemetryEvent::CallStarted { .. } => {
            metrics::counter!("booth_calls_started_total").increment(1);
        }
        TelemetryEvent::CallEnded { outcome, .. } => {
            metrics::counter!(
                "booth_calls_total",
                "outcome" => call_outcome_label(*outcome),
            )
            .increment(1);
        }
        TelemetryEvent::RecordingStopped { duration_ms, .. } => {
            metrics::histogram!("booth_recording_duration_seconds")
                .record(secs_from_millis(*duration_ms));
        }
        TelemetryEvent::UploadCompleted {
            duration_ms, bytes, ..
        } => {
            metrics::histogram!(
                "booth_upload_duration_seconds",
                "outcome" => "completed",
            )
            .record(secs_from_millis(*duration_ms));
            #[allow(clippy::cast_precision_loss)]
            metrics::histogram!("booth_upload_bytes").record(*bytes as f64);
        }
        TelemetryEvent::UploadFailed { .. } => {
            metrics::counter!(
                "booth_upload_failures_total",
                "reason" => "upload_failed",
            )
            .increment(1);
        }
        TelemetryEvent::RecordingStarted { .. }
        | TelemetryEvent::UploadStarted { .. }
        | TelemetryEvent::AudioDeviceChange { .. }
        | TelemetryEvent::GpioEdge(_)
        | TelemetryEvent::SystemSample { .. }
        | TelemetryEvent::Log { .. } => {
            // No counter for these in v1 — the structured event is the
            // authoritative log, and Prometheus dashboards key off the
            // companion events recorded above.
        }
    }
}

fn call_outcome_label(outcome: CallOutcome) -> &'static str {
    match outcome {
        CallOutcome::HungUpBeforeDial => "hung_up_before_dial",
        CallOutcome::HungUpDuringPrompt => "hung_up_during_prompt",
        CallOutcome::HungUpDuringRecording => "hung_up_during_recording",
        CallOutcome::HungUpDuringUpload => "hung_up_during_upload",
        CallOutcome::RecordingCompleted => "recording_completed",
        CallOutcome::RecordingFailed => "recording_failed",
        CallOutcome::UploadFailed => "upload_failed",
        CallOutcome::OperatorError => "operator_error",
        CallOutcome::Aborted => "aborted",
    }
}

#[allow(clippy::cast_precision_loss)]
fn secs_from_millis(ms: u64) -> f64 {
    ms as f64 / 1000.0
}

// ---------------------------------------------------------------------------
// Background task spawners
// ---------------------------------------------------------------------------

/// Spawn the periodic sampler task.
///
/// The task wakes every `config.interval`, takes a snapshot, updates
/// gauges, and publishes a [`TelemetryEvent::SystemSample`] on the bus
/// so subscribers (debug surface, operator forwarder) see it like any
/// other event.
pub fn spawn_system_sampler(
    sampler: SystemSampler,
    bus: TelemetryBus,
    config: SamplerConfig,
    start_instant: std::time::Instant,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let SamplerConfig { interval } = config;
        let mut ticker = tokio::time::interval(interval);
        // `Burst` would stack ticks on a slow consumer; we want at most
        // one tick per interval no matter how delayed the previous one
        // was.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let snapshot = sampler.sample_once();
            record_snapshot_gauges(&snapshot);
            let at_monotonic_ns = monotonic_ns_since(start_instant);
            bus.publish(TelemetryEvent::SystemSample {
                snapshot: Box::new(snapshot),
                at_monotonic_ns,
            });
        }
    })
}

/// Spawn the telemetry consumer that updates counters/histograms.
///
/// This subscribes to the bus once at startup; if the subscriber falls
/// behind, lagged batches are logged and the consumer resumes — metrics
/// are best-effort summaries, not the durable event log (which lives in
/// the operator forwarder added in PR `tb-runtime`).
pub fn spawn_telemetry_consumer(bus: &TelemetryBus, sampler: SystemSampler) -> JoinHandle<()> {
    let mut receiver = bus.subscribe();
    tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(record) => {
                    if let TelemetryEvent::AudioDeviceChange { name, channel } = &record.event {
                        sampler.record_audio_device(*channel, name.clone());
                    }
                    record_telemetry_event(&record.event);
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    debug!(
                        skipped,
                        "booth-metrics telemetry consumer lagged; metrics best-effort"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("booth-metrics telemetry consumer closed");
                    return;
                }
            }
        }
    })
}

fn monotonic_ns_since(start: std::time::Instant) -> u64 {
    let elapsed = start.elapsed();
    let secs = elapsed.as_secs();
    let nanos = u64::from(elapsed.subsec_nanos());
    secs.saturating_mul(1_000_000_000).saturating_add(nanos)
}

// ---------------------------------------------------------------------------
// Cross-platform / Pi-specific reads
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
fn read_cpu_temp_celsius() -> Option<f32> {
    let raw = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp").ok()?;
    let millis: i32 = raw.trim().parse().ok()?;
    #[allow(
        clippy::cast_precision_loss,
        reason = "thermal_zone values are well below f32 mantissa precision"
    )]
    let celsius = millis as f32 / 1000.0;
    Some(celsius)
}

#[cfg(not(target_os = "linux"))]
fn read_cpu_temp_celsius() -> Option<f32> {
    None
}

// ---------------------------------------------------------------------------
// Sysinfo → SystemSnapshot conversions
// ---------------------------------------------------------------------------

#[cfg(feature = "system")]
fn cpu_stats(system: &System) -> CpuStats {
    let per_core: Vec<f32> = system
        .cpus()
        .iter()
        .map(|cpu| (cpu.cpu_usage() / 100.0).clamp(0.0, 1.0))
        .collect();
    let usage_ratio = if per_core.is_empty() {
        0.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        let len = per_core.len() as f32;
        per_core.iter().copied().sum::<f32>() / len
    };
    let load = System::load_average();
    CpuStats {
        usage_ratio,
        per_core_usage_ratio: per_core,
        // Cast is sound: `cpus().len()` is bounded by the number of
        // cores reported by the OS, which fits in a u16 on any realistic
        // platform.
        #[allow(clippy::cast_possible_truncation)]
        physical_cores: system.cpus().len() as u16,
        load_avg_1m: load_avg_to_option(load.one),
        load_avg_5m: load_avg_to_option(load.five),
        load_avg_15m: load_avg_to_option(load.fifteen),
    }
}

#[cfg(feature = "system")]
#[allow(clippy::cast_possible_truncation)]
fn load_avg_to_option(value: f64) -> Option<f32> {
    if value.is_finite() {
        Some(value as f32)
    } else {
        None
    }
}

#[cfg(feature = "system")]
fn memory_stats(system: &System) -> MemoryStats {
    MemoryStats {
        total_bytes: system.total_memory(),
        used_bytes: system.used_memory(),
        swap_total_bytes: system.total_swap(),
        swap_used_bytes: system.used_swap(),
    }
}

#[cfg(feature = "system")]
fn disk_stats(disks: &Disks) -> Vec<DiskStats> {
    disks
        .list()
        .iter()
        .map(|disk| DiskStats {
            mount_point: disk.mount_point().to_string_lossy().into_owned(),
            filesystem: disk.file_system().to_string_lossy().into_owned(),
            total_bytes: disk.total_space(),
            available_bytes: disk.available_space(),
        })
        .collect()
}

#[cfg(feature = "system")]
fn network_stats(networks: &Networks) -> Vec<NetworkStats> {
    networks
        .iter()
        .map(|(name, data)| NetworkStats {
            interface: name.clone(),
            receive_bytes_total: data.total_received(),
            transmit_bytes_total: data.total_transmitted(),
        })
        .collect()
}

#[cfg(feature = "system")]
fn process_stats(system: &mut System) -> Option<ProcessStats> {
    let pid = sysinfo::get_current_pid().ok()?;
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::everything(),
    );
    let process = system.process(pid)?;
    Some(ProcessStats {
        resident_bytes: Some(process.memory()),
        virtual_bytes: Some(process.virtual_memory()),
        open_fds: None,
        threads: None,
        uptime_seconds: Some(process.run_time()),
    })
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests may panic on setup failure"
)]
mod tests {
    use super::*;
    use booth_hal::{AudioLevel, GpioEdge, PinRole};
    use std::time::Instant;

    fn ensure_registry() -> MetricsHandle {
        install_registry("test-booth").expect("install registry")
    }

    #[test]
    fn install_is_idempotent() {
        let a = ensure_registry();
        let b = install_registry("test-booth").expect("install again");
        assert_eq!(a.booth_id(), b.booth_id());
    }

    #[test]
    fn snapshot_has_some_populated_fields() {
        let sampler = SystemSampler::new();
        let snap = sampler.sample_once();
        // On macOS the temperature is None; on Linux it should usually
        // succeed but we only require that *something* populated.
        assert!(snap.cpu.is_some() || snap.memory.is_some());
        if let Some(mem) = &snap.memory {
            assert!(mem.total_bytes > 0, "total memory should be reported");
        }
    }

    #[test]
    fn audio_device_changes_propagate_into_snapshot() {
        let sampler = SystemSampler::new();
        sampler.record_audio_device(AudioChannel::Input, "Built-in Microphone".into());
        sampler.record_audio_device(AudioChannel::Output, "Built-in Output".into());
        let snap = sampler.sample_once();
        let audio = snap.audio.expect("audio populated");
        assert_eq!(audio.input_device.as_deref(), Some("Built-in Microphone"));
        assert_eq!(audio.output_device.as_deref(), Some("Built-in Output"));
    }

    #[test]
    fn record_telemetry_event_does_not_panic_on_every_variant() {
        ensure_registry();
        let variants = vec![
            TelemetryEvent::DigitDialed {
                digit: 7,
                pulses: 7,
                at_monotonic_ns: 0,
            },
            TelemetryEvent::StateTransition {
                from: "Idle".into(),
                to: "DialTone".into(),
                cause: "HookOff".into(),
                at_monotonic_ns: 0,
            },
            TelemetryEvent::AudioLevel(AudioLevel {
                channel: AudioChannel::Input,
                peak: 0.5,
                rms: 0.2,
                at_monotonic_ns: 0,
            }),
            TelemetryEvent::OperatorResponse {
                id: "req-1".into(),
                status: 200,
                duration_ms: 42,
            },
            TelemetryEvent::CallStarted {
                session_id: "s1".into(),
                at_monotonic_ns: 0,
            },
            TelemetryEvent::CallEnded {
                session_id: "s1".into(),
                outcome: CallOutcome::RecordingCompleted,
                at_monotonic_ns: 0,
            },
            TelemetryEvent::RecordingStopped {
                id: "rec-1".into(),
                session_id: "s1".into(),
                duration_ms: 12345,
                bytes: 65536,
                at_monotonic_ns: 0,
            },
            TelemetryEvent::UploadCompleted {
                recording_id: "rec-1".into(),
                session_id: "s1".into(),
                duration_ms: 800,
                bytes: 65536,
                at_monotonic_ns: 0,
            },
            TelemetryEvent::UploadFailed {
                recording_id: "rec-1".into(),
                session_id: "s1".into(),
                message: "503".into(),
                at_monotonic_ns: 0,
            },
            TelemetryEvent::GpioEdge(GpioEdge {
                role: PinRole::Hook,
                level: true,
                at_monotonic_ns: 0,
            }),
        ];
        for event in &variants {
            record_telemetry_event(event);
        }
    }

    #[test]
    fn render_includes_known_series_after_events() {
        let handle = ensure_registry();
        record_telemetry_event(&TelemetryEvent::CallEnded {
            session_id: "s1".into(),
            outcome: CallOutcome::RecordingCompleted,
            at_monotonic_ns: 0,
        });
        record_telemetry_event(&TelemetryEvent::DigitDialed {
            digit: 1,
            pulses: 1,
            at_monotonic_ns: 0,
        });
        let text = handle.render();
        assert!(
            text.contains("booth_calls_total"),
            "missing booth_calls_total in:\n{text}"
        );
        assert!(
            text.contains("booth_digits_dialed_total"),
            "missing booth_digits_dialed_total in:\n{text}"
        );
        // Global label is applied to every series.
        assert!(
            text.contains("booth_id=\"test-booth\""),
            "missing booth_id global label in:\n{text}"
        );
    }

    #[test]
    fn monotonic_ns_since_is_monotonic() {
        let start = Instant::now();
        let a = monotonic_ns_since(start);
        std::thread::sleep(Duration::from_millis(2));
        let b = monotonic_ns_since(start);
        assert!(b > a, "monotonic should advance: a={a}, b={b}");
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn spawn_system_sampler_publishes_to_bus() {
        let bus = TelemetryBus::new(8);
        let mut subscriber = bus.subscribe();
        let sampler = SystemSampler::new();
        let handle = spawn_system_sampler(
            sampler,
            bus,
            SamplerConfig {
                interval: Duration::from_millis(50),
            },
            Instant::now(),
        );
        tokio::time::advance(Duration::from_millis(60)).await;
        let record = tokio::time::timeout(Duration::from_secs(1), subscriber.recv())
            .await
            .expect("sampler should tick")
            .expect("bus delivers");
        assert!(matches!(record.event, TelemetryEvent::SystemSample { .. }));
        handle.abort();
    }
}
