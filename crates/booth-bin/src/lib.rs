//! Runtime wiring for the `telephone-booth` binary.
//!
//! This crate owns configuration loading, adapter construction, the async event
//! loop, and small diagnostics used by the CLI.

#![warn(missing_docs)]

use std::collections::HashSet;
use std::env;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use booth_core::{Effect, Event, PULSE_GROUP_TIMEOUT_MS, State, handle};
use booth_debug::{DebugConfig, DebugToken, RuntimeCommand};
use booth_hal::{
    AudioError, AudioRef, AudioSink, AudioSource, BuiltinTone, GpioEdge, GpioPort, OperatorClient,
    OperatorError, PinRole, RecordingId, TelemetryEvent,
};
use booth_pi::{AudioConfig, GpioConfig, GpioPull, OperatorConfig, PiAudioSink, PiAudioSource};
use booth_telemetry::TelemetryBus;
use observability::{ObservabilityConfig, SessionHandle};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

#[cfg(feature = "simulator")]
pub mod simulator;

pub mod file_storage;
pub mod observability;
pub mod pending_uploads;

/// Production config path used by the systemd service.
pub const DEFAULT_CONFIG_PATH: &str = "/etc/phone-booth/config.toml";

const DEV_CONFIG_PATH: &str = "./config.toml";
const EVENT_CHANNEL: usize = 256;
const EFFECT_CHANNEL: usize = 256;
const COMMAND_CHANNEL: usize = 64;
const OPERATOR_ATTEMPTS: u32 = 3;
const OPERATOR_BACKOFF_BASE: Duration = Duration::from_millis(100);

static OPERATOR_REQUEST_SEQ: AtomicU64 = AtomicU64::new(0);

/// Complete runtime configuration loaded from defaults, TOML, and environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    /// GPIO input pin assignments and electrical settings.
    pub gpio: GpioConfig,
    /// Audio device and recording settings.
    pub audio: AudioConfig,
    /// Operator API connection settings.
    pub operator: OperatorConfig,
    /// Embedded debug HTTP surface settings.
    pub debug: DebugConfig,
    /// Telemetry and logging settings owned by the binary.
    pub telemetry: TelemetryConfig,
    /// Observability stack (system metrics + operator event forwarding).
    pub observability: ObservabilityConfig,
    /// Debug bearer token loaded from `BOOTH_DEBUG_TOKEN` or `BOOTH_DEBUG_TOKEN_FILE`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_token: Option<String>,
}

impl RuntimeConfig {
    /// Return the replay ring size used by the telemetry bus.
    pub fn ring_buffer_capacity(&self) -> usize {
        self.debug.ring_buffer_capacity
    }

    /// Convert this runtime config back to the Pi adapter config.
    pub fn pi_config(&self) -> booth_pi::PiConfig {
        booth_pi::PiConfig {
            gpio: self.gpio.clone(),
            audio: self.audio.clone(),
            operator: self.operator.clone(),
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        let pi = booth_pi::PiConfig::default();
        Self {
            gpio: pi.gpio,
            audio: pi.audio,
            operator: pi.operator,
            debug: DebugConfig::default(),
            telemetry: TelemetryConfig::default(),
            observability: ObservabilityConfig::default(),
            debug_token: None,
        }
    }
}

/// Runtime-owned telemetry and logging config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    /// Default tracing filter used when `RUST_LOG` is unset.
    pub journal_level: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            journal_level: "info".to_string(),
        }
    }
}

/// Options that affect how the runtime task is spawned.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeOptions {
    /// Start the embedded debug surface task.
    pub start_debug: bool,
    /// Listen for SIGINT/SIGTERM and translate either into shutdown.
    pub listen_signals: bool,
    /// Send systemd readiness notification when the `systemd` feature is enabled.
    pub notify_systemd: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            start_debug: true,
            listen_signals: true,
            notify_systemd: true,
        }
    }
}

/// Object-safe HAL adapters consumed by the runtime.
pub struct RuntimeAdapters {
    gpio: Box<dyn GpioPort>,
    audio_sink: Box<dyn AudioSink>,
    audio_source: Box<dyn AudioSource>,
    operator: Arc<dyn OperatorClient>,
}

impl RuntimeAdapters {
    /// Build a runtime adapter bundle from trait objects.
    pub fn new(
        gpio: Box<dyn GpioPort>,
        audio_sink: Box<dyn AudioSink>,
        audio_source: Box<dyn AudioSource>,
        operator: Arc<dyn OperatorClient>,
    ) -> Self {
        Self {
            gpio,
            audio_sink,
            audio_source,
            operator,
        }
    }
}

/// Handle returned by [`spawn_runtime`].
pub struct RuntimeHandle {
    /// Sender for debug/runtime commands.
    pub commands: mpsc::Sender<RuntimeCommand>,
    /// Join handle that resolves to the final state when the runtime exits.
    pub join: JoinHandle<Result<State>>,
}

/// Additional handles returned for mock runtime construction.
#[cfg(feature = "mock")]
pub struct MockRuntimeHandles {
    /// GPIO injector for synthesizing hook and rotary edges.
    pub gpio: booth_mock::GpioInjector,
    /// Inspectable mock audio sink.
    pub audio_sink: booth_mock::MockAudioSink,
    /// Inspectable mock operator client.
    pub operator: booth_mock::MockOperatorClient,
}

/// Load the effective runtime config from defaults, an optional TOML file, and env overrides.
pub fn load_config(path: Option<&Path>) -> Result<RuntimeConfig> {
    let mut runtime = RuntimeConfig::default();
    if let Some(config_path) = config_path_to_read(path)? {
        let text = std::fs::read_to_string(&config_path)
            .with_context(|| format!("read config {}", config_path.display()))?;
        if !text.trim().is_empty() {
            runtime = toml::from_str(&text)
                .with_context(|| format!("parse config {}", config_path.display()))?;
        }
    }
    apply_env_overrides(&mut runtime)?;
    validate_config(&runtime)?;
    Ok(runtime)
}

/// Render a redacted TOML representation of the effective config.
pub fn render_config_toml(config: &RuntimeConfig) -> Result<String> {
    let mut redacted = config.clone();
    redacted.operator.token = redact_secret(&redacted.operator.token);
    redacted.debug_token = redacted.debug_token.as_deref().map(redact_secret);
    toml::to_string_pretty(&redacted).context("render config as TOML")
}

/// Validate config, then probe the concrete Pi adapters enough for ExecStartPre.
pub async fn check_runtime(config: &RuntimeConfig) -> Result<()> {
    validate_config(config)?;

    let _operator = booth_pi::PiOperatorClient::new(config.operator.clone())
        .map_err(|err| anyhow!("operator config invalid: {err}"))?;

    let mut sink = PiAudioSink::new(config.audio.clone());
    sink.stop()
        .await
        .map_err(|err| anyhow!("audio device check failed: {err}"))?;

    let _gpio = booth_pi::gpio::PiGpioPort::new(config.gpio.clone())
        .map_err(|err| anyhow!("gpio reservation check failed: {err}"))?;

    Ok(())
}

/// Run the pure state-machine diagnostic for `pulses` rotary pulses.
pub fn simulate_pulses(pulses: u8) -> Vec<(Event, State, Vec<Effect>)> {
    let mut state = State::Idle;
    let mut steps = Vec::new();
    for event in std::iter::once(Event::HookOff)
        .chain((0..pulses).map(|_| Event::RotaryPulse))
        .chain(std::iter::once(Event::Tick))
    {
        let (next, effects) = handle(state, event.clone());
        steps.push((event, next.clone(), effects));
        state = next;
    }
    steps
}

/// Build runtime adapters backed by the Raspberry Pi implementation.
pub fn build_pi_adapters(config: &RuntimeConfig, bus: &TelemetryBus) -> Result<RuntimeAdapters> {
    let (telemetry_tx, mut telemetry_rx) = mpsc::channel(128);
    let telemetry_bus = bus.clone();
    tokio::spawn(async move {
        while let Some(event) = telemetry_rx.recv().await {
            telemetry_bus.publish(event);
        }
    });

    let gpio = booth_pi::gpio::PiGpioPort::new(config.gpio.clone())
        .map_err(|err| anyhow!("open GPIO adapter: {err}"))?;
    let audio_sink = PiAudioSink::with_telemetry(config.audio.clone(), Some(telemetry_tx.clone()));

    let metadata_dir = metadata_dir_for(&config.audio.recordings_dir);
    let storage = file_storage::FileStorage::new(&metadata_dir)
        .map_err(|err| anyhow!("open file storage at {}: {err}", metadata_dir.display()))?;

    let audio_source =
        PiAudioSource::with_telemetry(config.audio.clone(), Arc::new(storage), Some(telemetry_tx));
    let operator = booth_pi::PiOperatorClient::new(config.operator.clone())
        .map_err(|err| anyhow!("create operator client: {err}"))?;

    Ok(RuntimeAdapters::new(
        Box::new(gpio),
        Box::new(audio_sink),
        Box::new(audio_source),
        Arc::new(operator),
    ))
}

/// Build runtime adapters backed by `booth-mock`.
#[cfg(feature = "mock")]
pub fn build_mock_adapters(bus: &TelemetryBus) -> (RuntimeAdapters, MockRuntimeHandles) {
    let (gpio, gpio_injector) = booth_mock::MockGpioPort::with_telemetry(bus);
    let audio_sink = booth_mock::MockAudioSink::with_telemetry(bus);
    let audio_source = booth_mock::MockAudioSource::with_telemetry(bus);
    let operator = booth_mock::MockOperatorClient::with_telemetry(bus);

    let adapters = RuntimeAdapters::new(
        Box::new(gpio),
        Box::new(audio_sink.clone()),
        Box::new(audio_source),
        Arc::new(operator.clone()),
    );
    let handles = MockRuntimeHandles {
        gpio: gpio_injector,
        audio_sink,
        operator,
    };
    (adapters, handles)
}

/// Build runtime adapters for the simulator TUI: a [`booth_mock::MockGpioPort`]
/// paired with either mock or real audio/operator adapters.
///
/// When `mock_io` is `true`, all adapters come from `booth-mock` (no audio
/// hardware or operator backend required). When `false`, the real
/// `booth-pi` audio and operator adapters are constructed — this is what
/// lets the simulator drive the actual cross-platform audio + HTTP code path
/// from a development machine.
///
/// Returns the [`RuntimeAdapters`] bundle plus the [`booth_mock::GpioInjector`]
/// the TUI uses to inject hook/rotary edges.
#[cfg(all(feature = "simulator", feature = "mock"))]
pub fn build_simulator_adapters(
    config: &RuntimeConfig,
    bus: &TelemetryBus,
    mock_io: bool,
) -> Result<(RuntimeAdapters, booth_mock::GpioInjector)> {
    let (gpio, gpio_injector) = booth_mock::MockGpioPort::with_telemetry(bus);

    let (audio_sink, audio_source, operator): (
        Box<dyn AudioSink>,
        Box<dyn AudioSource>,
        Arc<dyn OperatorClient>,
    ) = if mock_io {
        let sink = booth_mock::MockAudioSink::with_telemetry(bus);
        let source = booth_mock::MockAudioSource::with_telemetry(bus);
        let operator = booth_mock::MockOperatorClient::with_telemetry(bus);
        (Box::new(sink), Box::new(source), Arc::new(operator))
    } else {
        let (telemetry_tx, mut telemetry_rx) = mpsc::channel(128);
        let telemetry_bus = bus.clone();
        tokio::spawn(async move {
            while let Some(event) = telemetry_rx.recv().await {
                telemetry_bus.publish(event);
            }
        });
        let sink = PiAudioSink::with_telemetry(config.audio.clone(), Some(telemetry_tx.clone()));
        let metadata_dir = metadata_dir_for(&config.audio.recordings_dir);
        let storage = file_storage::FileStorage::new(&metadata_dir)
            .map_err(|err| anyhow!("open file storage at {}: {err}", metadata_dir.display()))?;
        let source = PiAudioSource::with_telemetry(
            config.audio.clone(),
            Arc::new(storage),
            Some(telemetry_tx),
        );
        let operator = booth_pi::PiOperatorClient::new(config.operator.clone())
            .map_err(|err| anyhow!("create operator client: {err}"))?;
        (Box::new(sink), Box::new(source), Arc::new(operator))
    };

    Ok((
        RuntimeAdapters::new(Box::new(gpio), audio_sink, audio_source, operator),
        gpio_injector,
    ))
}

/// Spawn the runtime loop and return its command sender and join handle.
pub fn spawn_runtime(
    config: RuntimeConfig,
    adapters: RuntimeAdapters,
    bus: TelemetryBus,
    options: RuntimeOptions,
) -> RuntimeHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_CHANNEL);
    let runtime_cmd_tx = cmd_tx.clone();
    let join = tokio::spawn(async move {
        run_runtime(config, adapters, bus, runtime_cmd_tx, cmd_rx, options).await
    });
    RuntimeHandle {
        commands: cmd_tx,
        join,
    }
}

async fn run_runtime(
    config: RuntimeConfig,
    adapters: RuntimeAdapters,
    bus: TelemetryBus,
    cmd_tx: mpsc::Sender<RuntimeCommand>,
    mut cmd_rx: mpsc::Receiver<RuntimeCommand>,
    options: RuntimeOptions,
) -> Result<State> {
    let RuntimeAdapters {
        gpio,
        audio_sink,
        audio_source,
        operator,
    } = adapters;

    let (event_tx, mut event_rx) = mpsc::channel::<Event>(EVENT_CHANNEL);
    let (effect_tx, effect_rx) = mpsc::channel::<Effect>(EFFECT_CHANNEL);
    let (audio_tx, audio_rx) = mpsc::channel::<AudioCommand>(32);
    let next_remote_audio = Arc::new(Mutex::new(None));
    let recordings_dir = PathBuf::from(config.audio.recordings_dir.clone());
    let spool_dir = pending_uploads_dir_for(&config.audio.recordings_dir);
    let upload_spool = match pending_uploads::PendingUploadSpool::open(&spool_dir) {
        Ok(spool) => Arc::new(spool),
        Err(err) => {
            warn!(dir = %spool_dir.display(), %err, "cannot open pending-upload spool; uploads will not be durable");
            Arc::new(
                pending_uploads::PendingUploadSpool::open(
                    std::env::temp_dir().join("phone-booth-spool"),
                )
                .map_err(|e| anyhow!("fallback spool: {e}"))?,
            )
        }
    };
    let session_handle = SessionHandle::default();

    // Install the Prometheus metrics registry (idempotent) and start the
    // background tasks for booth-metrics + the operator forwarder. All of
    // this is gated on `observability.enabled` so dev runs that don't
    // care about metrics can opt out.
    let mut observability_tasks: Vec<JoinHandle<()>> = Vec::new();
    let mut metrics_handle: Option<booth_metrics::MetricsHandle> = None;
    if config.observability.enabled {
        match booth_metrics::install_registry(config.observability.booth_id.clone()) {
            Ok(handle) => {
                metrics_handle = Some(handle);
                let sampler = booth_metrics::SystemSampler::new();
                let sampler_for_consumer = sampler.clone();
                let sampler_config = booth_metrics::SamplerConfig {
                    interval: Duration::from_millis(config.observability.sample_interval_ms),
                };
                let identity =
                    observability::RuntimeIdentity::new(config.observability.booth_id.clone());
                observability_tasks.push(booth_metrics::spawn_telemetry_consumer(
                    &bus,
                    sampler_for_consumer,
                ));
                observability_tasks.push(booth_metrics::spawn_system_sampler(
                    sampler,
                    bus.clone(),
                    sampler_config,
                    identity.start,
                ));
                if config.observability.operator_forward.enabled {
                    observability_tasks.push(observability::spawn_event_forwarder(
                        bus.clone(),
                        Arc::clone(&operator),
                        identity.clone(),
                        config.observability.clone(),
                        session_handle.clone(),
                    ));
                    observability_tasks.push(observability::spawn_system_pusher(
                        bus.clone(),
                        Arc::clone(&operator),
                        identity,
                        config.observability.clone(),
                    ));
                }
            }
            Err(err) => {
                warn!(%err, "failed to install metrics registry; observability disabled");
            }
        }
    }

    let gpio_task = tokio::spawn(gpio_task(gpio, event_tx.clone(), bus.clone()));
    let audio_task = tokio::spawn(audio_task(
        audio_sink,
        audio_rx,
        event_tx.clone(),
        bus.clone(),
    ));
    let effect_task = tokio::spawn(effect_task(
        effect_rx,
        audio_tx.clone(),
        audio_source,
        Arc::clone(&operator),
        event_tx.clone(),
        bus.clone(),
        Arc::clone(&next_remote_audio),
        recordings_dir.clone(),
        session_handle.clone(),
        Arc::clone(&upload_spool),
    ));

    let debug_handles = if options.start_debug {
        let mut debug_config = config.debug.clone();
        if let Some(token) = config.debug_token.clone() {
            debug_config.token = Some(DebugToken(token));
        }
        let debug_bus = bus.clone();
        let debug_cmd_tx = cmd_tx.clone();
        let metrics_render: Option<booth_debug::MetricsRender> =
            metrics_handle.as_ref().map(|handle| {
                let handle = handle.clone();
                let render: booth_debug::MetricsRender = Arc::new(move || handle.render());
                render
            });
        match booth_debug::serve_with_handles(debug_config, debug_bus, debug_cmd_tx, metrics_render)
            .await
        {
            Ok(handles) => Some(handles),
            Err(err) => {
                error!(%err, "debug surface failed to start");
                None
            }
        }
    } else {
        None
    };

    // Recover pending uploads from a previous run that was interrupted.
    {
        let pending = upload_spool.scan();
        if !pending.is_empty() {
            info!(
                count = pending.len(),
                "recovering pending uploads from spool"
            );
            for entry in pending {
                let operator = Arc::clone(&operator);
                let event_tx = event_tx.clone();
                let bus = bus.clone();
                let session_handle = session_handle.clone();
                let spool = Arc::clone(&upload_spool);
                tokio::spawn(async move {
                    let started = Instant::now();
                    let path = entry.path.clone();
                    let recording_id = entry.recording_id.clone();
                    let question_id = entry.question_id.clone().unwrap_or_default();
                    let bytes = tokio::fs::metadata(&path).await.map_or(0, |m| m.len());
                    let success = upload_recording(
                        &*operator,
                        &path,
                        &event_tx,
                        &bus,
                        recording_id.clone(),
                        question_id,
                        session_handle.current(),
                        started,
                        bytes,
                    )
                    .await;
                    if success {
                        spool.dequeue(&recording_id).ok();
                    }
                });
            }
        }
    }

    notify_ready(options.notify_systemd);

    let mut state = State::default();
    let mut shutdown = shutdown_signal(options.listen_signals);

    loop {
        tokio::select! {
            event = event_rx.recv() => {
                let Some(event) = event else {
                    warn!("event channel closed");
                    break;
                };
                handle_event(&mut state, event, &effect_tx, &bus).await?;
            }
            command = cmd_rx.recv() => {
                match command {
                    Some(RuntimeCommand::InjectEvent(event)) => {
                        if config.debug.allow_controls {
                            handle_event(&mut state, event, &effect_tx, &bus).await?;
                        } else {
                            bus.publish(TelemetryEvent::Error {
                                source: "booth_bin::debug".to_string(),
                                message: "debug controls are disabled".to_string(),
                            });
                        }
                    }
                    Some(RuntimeCommand::Snapshot(reply)) => {
                        let _ = reply.send(state.clone());
                    }
                    Some(RuntimeCommand::Shutdown) | None => break,
                }
            }
            () = &mut shutdown => {
                info!("shutdown signal received");
                break;
            }
        }
    }

    let _ = audio_tx.send(AudioCommand::Shutdown).await;
    gpio_task.abort();
    audio_task.abort();
    effect_task.abort();
    for task in observability_tasks {
        task.abort();
    }
    if let Some(handles) = debug_handles {
        // Signal graceful shutdown so listener tasks stop accepting connections.
        let _ = handles.shutdown_tx.send(());
        // Give listeners a moment to drain, then abort if they haven't stopped.
        let abort = handles.handle.abort_handle();
        let timeout = tokio::time::timeout(Duration::from_secs(5), handles.handle);
        if let Err(_elapsed) = timeout.await {
            abort.abort();
            tracing::warn!("debug server did not shut down within timeout, aborting");
        }
    }

    Ok(state)
}

async fn handle_event(
    state: &mut State,
    event: Event,
    effect_tx: &mpsc::Sender<Effect>,
    bus: &TelemetryBus,
) -> Result<()> {
    let from = state.clone();
    let (to, effects) = handle(from.clone(), event.clone());
    publish_transition(bus, &from, &to, &event);
    debug!(
        from = from.tag(),
        to = to.tag(),
        ?event,
        ?effects,
        "state transition"
    );
    *state = to;
    for effect in effects {
        effect_tx
            .send(effect)
            .await
            .context("effect dispatcher stopped")?;
    }
    Ok(())
}

fn publish_transition(bus: &TelemetryBus, from: &State, to: &State, event: &Event) {
    bus.publish(TelemetryEvent::StateTransition {
        from: from.tag().to_string(),
        to: to.tag().to_string(),
        cause: format!("{event:?}"),
        at_monotonic_ns: monotonic_ns(),
    });
}

async fn gpio_task(mut gpio: Box<dyn GpioPort>, event_tx: mpsc::Sender<Event>, bus: TelemetryBus) {
    loop {
        match gpio.next_edge().await {
            Ok(edge) => {
                bus.publish(TelemetryEvent::GpioEdge(edge));
                if let Some(event) = event_from_gpio(edge)
                    && event_tx.send(event).await.is_err()
                {
                    break;
                }
            }
            Err(err) => {
                bus.publish(TelemetryEvent::Error {
                    source: "gpio".to_string(),
                    message: err.to_string(),
                });
                warn!(%err, "gpio stream error");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

fn event_from_gpio(edge: GpioEdge) -> Option<Event> {
    match edge.role {
        PinRole::Hook => Some(if edge.level {
            Event::HookOn
        } else {
            Event::HookOff
        }),
        PinRole::RotaryPulse => (!edge.level).then_some(Event::RotaryPulse),
        PinRole::RotaryRead => None,
    }
}

async fn audio_task(
    mut sink: Box<dyn AudioSink>,
    mut rx: mpsc::Receiver<AudioCommand>,
    event_tx: mpsc::Sender<Event>,
    bus: TelemetryBus,
) {
    let mut playing = false;
    loop {
        if playing {
            tokio::select! {
                command = rx.recv() => {
                    match command {
                        Some(AudioCommand::Play(source)) => {
                            if let Err(err) = sink.stop().await {
                                publish_audio_error(&bus, &err);
                            }
                            match sink.play(source).await {
                                Ok(()) => playing = true,
                                Err(err) => {
                                    publish_audio_error(&bus, &err);
                                    playing = false;
                                }
                            }
                        }
                        Some(AudioCommand::Stop) => {
                            if let Err(err) = sink.stop().await {
                                publish_audio_error(&bus, &err);
                            }
                            playing = false;
                        }
                        Some(AudioCommand::Shutdown) | None => {
                            let _ = sink.stop().await;
                            break;
                        }
                    }
                }
                ended = sink.wait_for_end() => {
                    playing = false;
                    match ended {
                        Ok(()) => {
                            let _ = event_tx.send(Event::PlaybackEnded).await;
                        }
                        Err(err) => publish_audio_error(&bus, &err),
                    }
                }
            }
        } else {
            match rx.recv().await {
                Some(AudioCommand::Play(source)) => {
                    let completes = !matches!(source, AudioRef::Builtin(BuiltinTone::DialTone));
                    match sink.play(source).await {
                        Ok(()) => playing = completes,
                        Err(err) => publish_audio_error(&bus, &err),
                    }
                }
                Some(AudioCommand::Stop) => {
                    if let Err(err) = sink.stop().await {
                        publish_audio_error(&bus, &err);
                    }
                }
                Some(AudioCommand::Shutdown) | None => break,
            }
        }
    }
}

fn publish_audio_error(bus: &TelemetryBus, err: &AudioError) {
    bus.publish(TelemetryEvent::Error {
        source: "audio".to_string(),
        message: err.to_string(),
    });
    warn!(%err, "audio adapter error");
}

#[allow(clippy::too_many_arguments)]
async fn effect_task(
    mut effect_rx: mpsc::Receiver<Effect>,
    audio_tx: mpsc::Sender<AudioCommand>,
    mut audio_source: Box<dyn AudioSource>,
    operator: Arc<dyn OperatorClient>,
    event_tx: mpsc::Sender<Event>,
    bus: TelemetryBus,
    next_remote_audio: Arc<Mutex<Option<AudioRef>>>,
    recordings_dir: PathBuf,
    session_handle: SessionHandle,
    upload_spool: Arc<pending_uploads::PendingUploadSpool>,
) {
    let mut pulse_timeout: Option<JoinHandle<()>> = None;
    while let Some(effect) = effect_rx.recv().await {
        match effect {
            Effect::Play(source) => {
                let source = resolve_audio_ref(source, &next_remote_audio).await;
                let _ = audio_tx.send(AudioCommand::Play(source)).await;
            }
            Effect::StopAudio => {
                let _ = audio_tx.send(AudioCommand::Stop).await;
            }
            Effect::StartRecording => {
                if let Err(err) = audio_source.start().await {
                    publish_audio_error(&bus, &err);
                }
            }
            Effect::StopRecording => match audio_source.stop().await {
                Ok(Some(recording_id)) => {
                    if let Some(session_id) = session_handle.current() {
                        let (duration_ms, bytes) = recording_size(&*audio_source, &recording_id)
                            .await
                            .unwrap_or((0, 0));
                        bus.publish(TelemetryEvent::RecordingStopped {
                            id: recording_id.clone(),
                            session_id,
                            duration_ms,
                            bytes,
                            at_monotonic_ns: monotonic_ns(),
                        });
                    }
                    let _ = event_tx
                        .send(Event::RecordingFinished { recording_id })
                        .await;
                }
                Ok(None) => {}
                Err(err) => publish_audio_error(&bus, &err),
            },
            Effect::UploadRecording {
                recording_id,
                question_id,
            } => {
                let session_id = session_handle.current();
                if let Some(sid) = session_id.clone() {
                    bus.publish(TelemetryEvent::UploadStarted {
                        recording_id: recording_id.clone(),
                        session_id: sid,
                        at_monotonic_ns: monotonic_ns(),
                    });
                }
                // Resolve the file path and enqueue in the durable spool
                // before attempting the upload.
                let path = match audio_source.path_of(&recording_id).await {
                    Ok(p) => p,
                    Err(err) => {
                        publish_audio_error(&bus, &err);
                        continue;
                    }
                };
                let spool_entry = pending_uploads::SpoolEntry {
                    recording_id: recording_id.clone(),
                    question_id: Some(question_id.clone()),
                    path: path.clone(),
                };
                if let Err(err) = upload_spool.enqueue(&spool_entry) {
                    warn!(%err, "failed to write upload spool entry; upload will not survive crash");
                }
                let started = Instant::now();
                let bytes = recording_size(&*audio_source, &recording_id)
                    .await
                    .map_or(0, |(_, b)| b);
                let success = upload_recording(
                    &*operator,
                    &path,
                    &event_tx,
                    &bus,
                    recording_id.clone(),
                    question_id,
                    session_id,
                    started,
                    bytes,
                )
                .await;
                // Only dequeue on success; on failure the spool entry remains
                // so it can be retried on next startup.
                if success {
                    upload_spool.dequeue(&recording_id).ok();
                }
            }
            Effect::FetchRandomQuestion => {
                fetch_random_question(
                    &*operator,
                    &event_tx,
                    &bus,
                    &next_remote_audio,
                    &recordings_dir,
                )
                .await;
            }
            Effect::FetchRandomMessage => {
                fetch_random_message(
                    &*operator,
                    &event_tx,
                    &bus,
                    &next_remote_audio,
                    &recordings_dir,
                )
                .await;
            }
            Effect::PutStatus(status) => {
                if let Err(err) =
                    retry_operator("PUT /v1/status", &bus, || operator.put_status(status)).await
                {
                    publish_operator_error(&bus, "put_status", &err);
                }
            }
            Effect::ArmPulseTimeout => {
                if let Some(task) = pulse_timeout.take() {
                    task.abort();
                }
                let tx = event_tx.clone();
                pulse_timeout = Some(tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(PULSE_GROUP_TIMEOUT_MS)).await;
                    let _ = tx.send(Event::Tick).await;
                }));
            }
            Effect::CancelPulseTimeout => {
                if let Some(task) = pulse_timeout.take() {
                    task.abort();
                }
            }
            Effect::Log { message } => {
                info!(%message, "state-machine log");
                bus.publish(TelemetryEvent::Log {
                    level: "info".to_string(),
                    target: "booth_core".to_string(),
                    message,
                });
            }
        }
    }
}

async fn resolve_audio_ref(
    source: AudioRef,
    next_remote_audio: &Arc<Mutex<Option<AudioRef>>>,
) -> AudioRef {
    match source {
        AudioRef::RemoteUrl(url) if url.is_empty() => next_remote_audio
            .lock()
            .await
            .take()
            .unwrap_or(AudioRef::RemoteUrl(url)),
        other => other,
    }
}

fn operator_audio_ref(
    audio_url: String,
    audio_sha256: Option<&str>,
    recordings_dir: &Path,
) -> AudioRef {
    if let Some(sha256) = audio_sha256
        && is_sha256_hex(sha256)
    {
        let local_path = recordings_dir.join(format!("{sha256}.flac"));
        if local_path.is_file() {
            debug!(
                path = %local_path.display(),
                "using local operator audio instead of remote URL"
            );
            return AudioRef::LocalFile(local_path.to_string_lossy().into_owned());
        }
    }
    AudioRef::RemoteUrl(audio_url)
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

async fn fetch_random_question(
    operator: &dyn OperatorClient,
    event_tx: &mpsc::Sender<Event>,
    bus: &TelemetryBus,
    next_remote_audio: &Arc<Mutex<Option<AudioRef>>>,
    recordings_dir: &Path,
) {
    match retry_operator("GET /v1/questions/random", bus, || {
        operator.random_question()
    })
    .await
    {
        Ok(question) => {
            *next_remote_audio.lock().await = Some(operator_audio_ref(
                question.audio_url,
                question.audio_sha256.as_deref(),
                recordings_dir,
            ));
            let _ = event_tx
                .send(Event::QuestionReady {
                    question_id: question.id,
                })
                .await;
        }
        Err(err) => {
            publish_operator_error(bus, "random_question", &err);
            let _ = event_tx
                .send(Event::QuestionFailed {
                    reason: err.to_string(),
                })
                .await;
        }
    }
}

async fn fetch_random_message(
    operator: &dyn OperatorClient,
    event_tx: &mpsc::Sender<Event>,
    bus: &TelemetryBus,
    next_remote_audio: &Arc<Mutex<Option<AudioRef>>>,
    recordings_dir: &Path,
) {
    match retry_operator("GET /v1/messages/random", bus, || operator.random_message()).await {
        Ok(message) => {
            *next_remote_audio.lock().await = Some(operator_audio_ref(
                message.audio_url,
                message.audio_sha256.as_deref(),
                recordings_dir,
            ));
            let _ = event_tx.send(Event::MessageReady).await;
        }
        Err(err) => {
            publish_operator_error(bus, "random_message", &err);
            let _ = event_tx
                .send(Event::MessageFailed {
                    reason: err.to_string(),
                })
                .await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn upload_recording(
    operator: &dyn OperatorClient,
    path: &str,
    event_tx: &mpsc::Sender<Event>,
    bus: &TelemetryBus,
    recording_id: RecordingId,
    question_id: booth_hal::QuestionId,
    session_id: Option<String>,
    started: Instant,
    bytes: u64,
) -> bool {
    let result = async {
        let slot = retry_operator("POST /v1/uploads", bus, || {
            operator.init_upload(Some(&question_id))
        })
        .await?;
        retry_operator("PUT <presigned-upload-url>", bus, || {
            operator.put_upload(&slot, path)
        })
        .await?;
        retry_operator("POST /v1/uploads/{id}/complete", bus, || {
            operator.complete_upload(&slot.id, &recording_id, 0)
        })
        .await?;
        Ok::<(), OperatorError>(())
    }
    .await;

    let duration_ms = elapsed_ms(started);
    match result {
        Ok(()) => {
            if let Some(sid) = session_id {
                bus.publish(TelemetryEvent::UploadCompleted {
                    recording_id: recording_id.clone(),
                    session_id: sid,
                    duration_ms,
                    bytes,
                    at_monotonic_ns: monotonic_ns(),
                });
            }
            if let Err(err) = audio_source.cleanup_recording(&recording_id).await {
                warn!(%recording_id, %err, "failed to clean up recording metadata");
            }
            let _ = event_tx.send(Event::UploadComplete).await;
            true
        }
        Err(err) => {
            publish_operator_error(bus, "upload_recording", &err);
            if let Some(sid) = session_id {
                bus.publish(TelemetryEvent::UploadFailed {
                    recording_id: recording_id.clone(),
                    session_id: sid,
                    message: err.to_string(),
                    at_monotonic_ns: monotonic_ns(),
                });
            }
            let _ = event_tx
                .send(Event::UploadFailed {
                    reason: err.to_string(),
                })
                .await;
            false
        }
    }
}

async fn retry_operator<T, F, Fut>(
    route: &str,
    bus: &TelemetryBus,
    mut operation: F,
) -> StdResult<T, OperatorError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = StdResult<T, OperatorError>>,
{
    for attempt in 1..=OPERATOR_ATTEMPTS {
        let request_id = format!(
            "runtime-{}",
            OPERATOR_REQUEST_SEQ.fetch_add(1, Ordering::Relaxed) + 1
        );
        bus.publish(TelemetryEvent::OperatorRequest {
            id: request_id.clone(),
            route: route.to_string(),
        });
        let started = Instant::now();
        let result = operation().await;
        bus.publish(TelemetryEvent::OperatorResponse {
            id: request_id,
            status: operator_status(&result),
            duration_ms: elapsed_ms(started),
        });

        match result {
            Ok(value) => return Ok(value),
            Err(err) if attempt == OPERATOR_ATTEMPTS || !is_retryable_operator_error(&err) => {
                return Err(err);
            }
            Err(err) => {
                publish_operator_error(bus, route, &err);
                tokio::time::sleep(operator_backoff(attempt)).await;
            }
        }
    }

    Err(OperatorError::Transport(
        "operator retry loop exhausted".into(),
    ))
}

fn operator_status<T>(result: &StdResult<T, OperatorError>) -> u16 {
    match result {
        Ok(_) => 200,
        Err(OperatorError::Auth(_) | OperatorError::Unauthorized(_)) => 401,
        Err(OperatorError::DuplicateRecording(_)) => 409,
        Err(OperatorError::Protocol(_)) => 502,
        Err(OperatorError::Transport(_) | OperatorError::Unsupported(_)) => 503,
        Err(OperatorError::Server { status, .. }) => *status,
    }
}

fn is_retryable_operator_error(err: &OperatorError) -> bool {
    matches!(err, OperatorError::Transport(_))
        || matches!(err, OperatorError::Server { status, .. } if *status >= 500)
}

fn operator_backoff(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1);
    let multiplier = 1_u32.checked_shl(shift).unwrap_or(u32::MAX);
    OPERATOR_BACKOFF_BASE.saturating_mul(multiplier)
}

fn publish_operator_error(bus: &TelemetryBus, source: &str, err: &OperatorError) {
    bus.publish(TelemetryEvent::Error {
        source: source.to_string(),
        message: err.to_string(),
    });
    warn!(%source, %err, "operator adapter error");
}

#[derive(Debug)]
enum AudioCommand {
    Play(AudioRef),
    Stop,
    Shutdown,
}

/// Upper bound for `max_recording_secs` validated at startup.
const MAX_RECORDING_SECS_CEILING: u32 = 600;

fn validate_config(config: &RuntimeConfig) -> Result<()> {
    if config.operator.base_url.trim().is_empty() {
        bail!("operator.base_url must not be empty");
    }
    if config.audio.channels == 0 {
        bail!("audio.channels must be at least 1");
    }
    if config.audio.sample_rate_hz == 0 {
        bail!("audio.sample_rate_hz must be at least 1");
    }

    // --- Operator timeout / backoff bounds ---
    if config.operator.http_timeout_secs == 0 {
        bail!("operator.http_timeout_secs must be greater than 0");
    }
    if config.operator.ws_reconnect_initial_ms == 0 {
        bail!("operator.ws_reconnect_initial_ms must be greater than 0");
    }
    if config.operator.ws_reconnect_max_ms < config.operator.ws_reconnect_initial_ms {
        bail!(
            "operator.ws_reconnect_max_ms ({}) must be >= operator.ws_reconnect_initial_ms ({})",
            config.operator.ws_reconnect_max_ms,
            config.operator.ws_reconnect_initial_ms
        );
    }

    // --- Audio recording duration ---
    if config.audio.max_recording_secs == 0 {
        bail!("audio.max_recording_secs must be greater than 0");
    }
    if config.audio.max_recording_secs > MAX_RECORDING_SECS_CEILING {
        bail!(
            "audio.max_recording_secs ({}) exceeds maximum allowed ({})",
            config.audio.max_recording_secs,
            MAX_RECORDING_SECS_CEILING
        );
    }

    // --- Observability interval / buffer bounds ---
    if config.observability.sample_interval_ms == 0 {
        bail!("observability.sample_interval_ms must be greater than 0");
    }
    let fwd = &config.observability.operator_forward;
    if fwd.batch_max == 0 {
        bail!("observability.operator_forward.batch_max must be greater than 0");
    }
    if fwd.flush_interval_ms == 0 {
        bail!("observability.operator_forward.flush_interval_ms must be greater than 0");
    }
    if fwd.buffer_max < fwd.batch_max {
        bail!(
            "observability.operator_forward.buffer_max ({}) must be >= observability.operator_forward.batch_max ({})",
            fwd.buffer_max,
            fwd.batch_max
        );
    }
    if fwd.system_push_interval_ms == 0 {
        bail!("observability.operator_forward.system_push_interval_ms must be greater than 0");
    }

    let pins = [
        config.gpio.hook,
        config.gpio.rotary_pulse,
        config.gpio.rotary_read,
    ];
    let unique: HashSet<u8> = pins.into_iter().collect();
    if unique.len() != pins.len() {
        bail!("gpio pins must be unique");
    }
    Ok(())
}

fn apply_env_overrides(config: &mut RuntimeConfig) -> Result<()> {
    if let Some(value) = env::var_os("BOOTH_OPERATOR_BASE_URL") {
        config.operator.base_url = value.to_string_lossy().into_owned();
    }
    if let Some(value) = secret_env("BOOTH_OPERATOR_TOKEN", "BOOTH_OPERATOR_TOKEN_FILE")? {
        config.operator.token = value;
    }
    if let Some(value) = secret_env("BOOTH_DEBUG_TOKEN", "BOOTH_DEBUG_TOKEN_FILE")? {
        config.debug_token = Some(value);
    }
    if let Some(value) = env::var_os("BOOTH_AUDIO_DEVICE") {
        config.audio.device_substring = Some(value.to_string_lossy().into_owned());
    }

    set_gpio_u8(
        &mut config.gpio.hook,
        &["BOOTH_GPIO_HOOK", "BOOTH_GPIO_HOOK_BCM"],
    )?;
    set_gpio_u8(
        &mut config.gpio.rotary_pulse,
        &["BOOTH_GPIO_ROTARY_PULSE", "BOOTH_GPIO_ROTARY_PULSE_BCM"],
    )?;
    set_gpio_u8(
        &mut config.gpio.rotary_read,
        &[
            "BOOTH_GPIO_ROTARY_READ",
            "BOOTH_GPIO_ROTARY_READ_BCM",
            "BOOTH_GPIO_ROTARY_GATE",
            "BOOTH_GPIO_ROTARY_GATE_BCM",
        ],
    )?;
    set_gpio_u64(&mut config.gpio.debounce_ms, &["BOOTH_GPIO_DEBOUNCE_MS"])?;
    if let Some(value) = env::var_os("BOOTH_GPIO_PULL") {
        config.gpio.pull = parse_pull(&value.to_string_lossy())?;
    }
    set_gpio_bool(&mut config.gpio.invert.hook, &["BOOTH_GPIO_INVERT_HOOK"])?;
    set_gpio_bool(
        &mut config.gpio.invert.rotary_pulse,
        &["BOOTH_GPIO_INVERT_ROTARY_PULSE"],
    )?;
    set_gpio_bool(
        &mut config.gpio.invert.rotary_read,
        &[
            "BOOTH_GPIO_INVERT_ROTARY_READ",
            "BOOTH_GPIO_INVERT_ROTARY_GATE",
        ],
    )?;

    if let Some(value) = env::var_os("BOOTH_OBSERVABILITY_ENABLED") {
        config.observability.enabled =
            parse_bool(&value.to_string_lossy()).context("parse BOOTH_OBSERVABILITY_ENABLED")?;
    }
    if let Some(value) = env::var_os("BOOTH_OBSERVABILITY_BOOTH_ID") {
        config.observability.booth_id = value.to_string_lossy().into_owned();
    }
    if let Some(value) = env::var_os("BOOTH_OBSERVABILITY_FORWARD_ENABLED") {
        config.observability.operator_forward.enabled = parse_bool(&value.to_string_lossy())
            .context("parse BOOTH_OBSERVABILITY_FORWARD_ENABLED")?;
    }

    Ok(())
}

fn config_path_to_read(path: Option<&Path>) -> Result<Option<PathBuf>> {
    if let Some(path) = path {
        if !path.exists() {
            bail!("config file does not exist: {}", path.display());
        }
        return Ok(Some(path.to_path_buf()));
    }

    let default = Path::new(DEFAULT_CONFIG_PATH);
    if default.exists() {
        return Ok(Some(default.to_path_buf()));
    }
    let dev = Path::new(DEV_CONFIG_PATH);
    if dev.exists() {
        return Ok(Some(dev.to_path_buf()));
    }
    Ok(None)
}

fn secret_env(value_key: &str, file_key: &str) -> Result<Option<String>> {
    if let Some(value) = env::var_os(value_key) {
        return Ok(Some(value.to_string_lossy().into_owned()));
    }
    if let Some(path) = env::var_os(file_key) {
        let path = PathBuf::from(path);
        let value = std::fs::read_to_string(&path)
            .with_context(|| format!("read secret from {}", path.display()))?;
        return Ok(Some(value.trim_end_matches(['\r', '\n']).to_string()));
    }
    Ok(None)
}

fn set_gpio_u8(target: &mut u8, keys: &[&str]) -> Result<()> {
    for key in keys {
        if let Some(value) = env::var_os(key) {
            *target = value
                .to_string_lossy()
                .parse()
                .with_context(|| format!("parse {key} as u8"))?;
        }
    }
    Ok(())
}

fn set_gpio_u64(target: &mut u64, keys: &[&str]) -> Result<()> {
    for key in keys {
        if let Some(value) = env::var_os(key) {
            *target = value
                .to_string_lossy()
                .parse()
                .with_context(|| format!("parse {key} as u64"))?;
        }
    }
    Ok(())
}

fn set_gpio_bool(target: &mut bool, keys: &[&str]) -> Result<()> {
    for key in keys {
        if let Some(value) = env::var_os(key) {
            *target =
                parse_bool(&value.to_string_lossy()).with_context(|| format!("parse {key}"))?;
        }
    }
    Ok(())
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => bail!("expected boolean, got {value}"),
    }
}

fn parse_pull(value: &str) -> Result<GpioPull> {
    match value.trim().to_ascii_lowercase().as_str() {
        "up" => Ok(GpioPull::Up),
        "down" => Ok(GpioPull::Down),
        _ => bail!("expected up or down, got {value}"),
    }
}

fn redact_secret(secret: &str) -> String {
    if secret.is_empty() {
        return "<empty>".to_string();
    }
    let mut last_four = secret.chars().rev().take(4).collect::<Vec<_>>();
    last_four.reverse();
    format!("<redacted:{}>", last_four.into_iter().collect::<String>())
}

fn monotonic_ns() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let nanos = START.get_or_init(Instant::now).elapsed().as_nanos();
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

async fn recording_size(
    audio_source: &dyn AudioSource,
    recording_id: &RecordingId,
) -> Option<(u64, u64)> {
    // Best-effort: look up the recording's on-disk path and report its
    // file size. Duration is left as 0 because the adapter doesn't expose
    // it without re-decoding the FLAC stream — Grafana derives it from
    // CallStarted → RecordingStopped instead.
    let path = audio_source.path_of(recording_id).await.ok()?;
    let bytes = tokio::fs::metadata(&path).await.ok()?.len();
    Some((0, bytes))
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn shutdown_signal(listen: bool) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    if !listen {
        return Box::pin(std::future::pending());
    }
    Box::pin(async move {
        #[cfg(unix)]
        {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    if let Err(err) = result {
                        warn!(%err, "failed to listen for ctrl-c");
                    }
                }
                () = async {
                    if let Some(signal) = &mut sigterm {
                        let _ = signal.recv().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {}
            }
        }
        #[cfg(not(unix))]
        {
            if let Err(err) = tokio::signal::ctrl_c().await {
                warn!(%err, "failed to listen for ctrl-c");
            }
        }
    })
}

fn notify_ready(enabled: bool) {
    if !enabled {
        return;
    }
    #[cfg(all(feature = "systemd", unix))]
    {
        if let Err(err) = send_systemd_notify("READY=1") {
            warn!(%err, "failed to notify systemd readiness");
        }
    }
    #[cfg(not(all(feature = "systemd", unix)))]
    {
        if env::var_os("NOTIFY_SOCKET").is_some() {
            debug!("NOTIFY_SOCKET set but booth-bin was built without the systemd feature");
        }
    }
}

/// Derive the metadata storage directory from the recordings directory.
///
/// Places it as a sibling: `<recordings_dir>/../metadata/`.
fn metadata_dir_for(recordings_dir: &str) -> PathBuf {
    Path::new(recordings_dir)
        .parent()
        .unwrap_or_else(|| Path::new(recordings_dir))
        .join("metadata")
}

/// Derive the pending-uploads spool directory from the recordings directory.
///
/// Places it as a sibling: `<recordings_dir>/../pending-uploads/`.
fn pending_uploads_dir_for(recordings_dir: &str) -> PathBuf {
    Path::new(recordings_dir)
        .parent()
        .unwrap_or_else(|| Path::new(recordings_dir))
        .join("pending-uploads")
}

#[cfg(all(feature = "systemd", unix))]
fn send_systemd_notify(message: &str) -> std::io::Result<()> {
    use std::os::unix::net::UnixDatagram;

    let Some(socket) = env::var_os("NOTIFY_SOCKET") else {
        return Ok(());
    };
    let socket = socket.to_string_lossy();
    let target = socket.strip_prefix('@').map_or_else(
        || socket.clone().into_owned(),
        |stripped| {
            let mut abstract_name = String::from("\0");
            abstract_name.push_str(stripped);
            abstract_name
        },
    );
    let datagram = UnixDatagram::unbound()?;
    datagram.send_to(message.as_bytes(), target)?;
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests may panic on setup failure"
)]
mod tests {
    use super::{AudioRef, RuntimeConfig, is_sha256_hex, operator_audio_ref, validate_config};
    use std::fs;

    #[test]
    fn operator_audio_ref_uses_local_sha_file_when_present() -> std::io::Result<()> {
        let sha = "a".repeat(64);
        let recordings_dir = unique_temp_dir();
        fs::create_dir_all(&recordings_dir)?;
        let local_file = recordings_dir.join(format!("{sha}.flac"));
        fs::write(&local_file, b"flac")?;

        let audio = operator_audio_ref(
            "https://operator.example/audio.flac".to_string(),
            Some(&sha),
            &recordings_dir,
        );

        assert_eq!(
            audio,
            AudioRef::LocalFile(local_file.to_string_lossy().into_owned())
        );
        fs::remove_dir_all(recordings_dir)?;
        Ok(())
    }

    #[test]
    fn operator_audio_ref_falls_back_to_remote_when_local_file_is_absent() {
        let recordings_dir = unique_temp_dir();
        let remote = "https://operator.example/audio.flac".to_string();

        let audio = operator_audio_ref(remote.clone(), Some(&"b".repeat(64)), &recordings_dir);

        assert_eq!(audio, AudioRef::RemoteUrl(remote));
    }

    #[test]
    fn operator_audio_ref_falls_back_to_remote_when_sha_is_invalid() {
        let recordings_dir = unique_temp_dir();
        let remote = "https://operator.example/audio.flac".to_string();

        let audio = operator_audio_ref(remote.clone(), Some("../not-a-sha"), &recordings_dir);

        assert_eq!(audio, AudioRef::RemoteUrl(remote));
    }

    #[test]
    fn sha_validation_accepts_lowercase_hex_only() {
        assert!(is_sha256_hex(&"0".repeat(64)));
        assert!(!is_sha256_hex(&"A".repeat(64)));
        assert!(!is_sha256_hex(&"g".repeat(64)));
        assert!(!is_sha256_hex(&"0".repeat(63)));
    }

    fn unique_temp_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("telephone-booth-test-{}", uuid::Uuid::new_v4()))
    }

    // --- validate_config tests ---

    #[test]
    fn default_config_passes_validation() {
        let config = RuntimeConfig::default();
        validate_config(&config).expect("default config should be valid");
    }

    #[test]
    fn rejects_zero_http_timeout() {
        let mut config = RuntimeConfig::default();
        config.operator.http_timeout_secs = 0;
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("http_timeout_secs"));
    }

    #[test]
    fn rejects_zero_ws_reconnect_initial() {
        let mut config = RuntimeConfig::default();
        config.operator.ws_reconnect_initial_ms = 0;
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("ws_reconnect_initial_ms"));
    }

    #[test]
    fn rejects_inverted_ws_reconnect_bounds() {
        let mut config = RuntimeConfig::default();
        config.operator.ws_reconnect_initial_ms = 5_000;
        config.operator.ws_reconnect_max_ms = 1_000;
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("ws_reconnect_max_ms"));
    }

    #[test]
    fn accepts_equal_ws_reconnect_bounds() {
        let mut config = RuntimeConfig::default();
        config.operator.ws_reconnect_initial_ms = 2_000;
        config.operator.ws_reconnect_max_ms = 2_000;
        validate_config(&config).expect("equal bounds should be valid");
    }

    #[test]
    fn rejects_zero_max_recording_secs() {
        let mut config = RuntimeConfig::default();
        config.audio.max_recording_secs = 0;
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("max_recording_secs"));
    }

    #[test]
    fn rejects_excessive_max_recording_secs() {
        let mut config = RuntimeConfig::default();
        config.audio.max_recording_secs = 601;
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("max_recording_secs"));
    }

    #[test]
    fn accepts_max_recording_at_ceiling() {
        let mut config = RuntimeConfig::default();
        config.audio.max_recording_secs = 600;
        validate_config(&config).expect("600s should be valid");
    }

    #[test]
    fn rejects_zero_sample_interval() {
        let mut config = RuntimeConfig::default();
        config.observability.sample_interval_ms = 0;
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("sample_interval_ms"));
    }

    #[test]
    fn rejects_zero_batch_max() {
        let mut config = RuntimeConfig::default();
        config.observability.operator_forward.batch_max = 0;
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("batch_max"));
    }

    #[test]
    fn rejects_zero_flush_interval() {
        let mut config = RuntimeConfig::default();
        config.observability.operator_forward.flush_interval_ms = 0;
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("flush_interval_ms"));
    }

    #[test]
    fn rejects_buffer_max_less_than_batch_max() {
        let mut config = RuntimeConfig::default();
        config.observability.operator_forward.batch_max = 100;
        config.observability.operator_forward.buffer_max = 50;
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("buffer_max"));
    }

    #[test]
    fn accepts_buffer_max_equal_to_batch_max() {
        let mut config = RuntimeConfig::default();
        config.observability.operator_forward.batch_max = 100;
        config.observability.operator_forward.buffer_max = 100;
        validate_config(&config).expect("equal buffer/batch should be valid");
    }

    #[test]
    fn rejects_zero_system_push_interval() {
        let mut config = RuntimeConfig::default();
        config
            .observability
            .operator_forward
            .system_push_interval_ms = 0;
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("system_push_interval_ms"));
    }
}
