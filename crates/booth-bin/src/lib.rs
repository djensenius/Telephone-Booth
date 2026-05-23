//! Runtime wiring for the `telephone-booth` binary.
//!
//! This crate owns configuration loading, adapter construction, the async event
//! loop, and small diagnostics used by the CLI.

#![warn(missing_docs)]

use std::collections::{HashMap, HashSet};
use std::env;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use booth_core::{Effect, Event, PULSE_GROUP_TIMEOUT_MS, State, handle};
use booth_debug::{DebugConfig, RuntimeCommand};
use booth_hal::{
    AudioError, AudioRef, AudioSink, AudioSource, BuiltinTone, GpioEdge, GpioPort, OperatorClient,
    OperatorError, PinRole, RecordingId, Storage, StorageError, TelemetryEvent,
};
use booth_pi::{AudioConfig, GpioConfig, GpioPull, OperatorConfig, PiAudioSink, PiAudioSource};
use booth_telemetry::TelemetryBus;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

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
    let audio_source = PiAudioSource::with_telemetry(
        config.audio.clone(),
        Arc::new(MemoryStorage::default()),
        Some(telemetry_tx),
    );
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
    ));

    let debug_task = if options.start_debug {
        let debug_config = config.debug.clone();
        let debug_bus = bus.clone();
        let debug_cmd_tx = cmd_tx.clone();
        Some(tokio::spawn(async move {
            if let Err(err) = booth_debug::serve(debug_config, debug_bus, debug_cmd_tx).await {
                error!(%err, "debug surface stopped");
            }
        }))
    } else {
        None
    };

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
    if let Some(task) = debug_task {
        task.abort();
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

async fn effect_task(
    mut effect_rx: mpsc::Receiver<Effect>,
    audio_tx: mpsc::Sender<AudioCommand>,
    mut audio_source: Box<dyn AudioSource>,
    operator: Arc<dyn OperatorClient>,
    event_tx: mpsc::Sender<Event>,
    bus: TelemetryBus,
    next_remote_audio: Arc<Mutex<Option<String>>>,
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
                upload_recording(
                    &*operator,
                    &*audio_source,
                    &event_tx,
                    &bus,
                    recording_id,
                    question_id,
                )
                .await;
            }
            Effect::FetchRandomQuestion => {
                fetch_random_question(&*operator, &event_tx, &bus, &next_remote_audio).await;
            }
            Effect::FetchRandomMessage => {
                fetch_random_message(&*operator, &event_tx, &bus, &next_remote_audio).await;
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
    next_remote_audio: &Arc<Mutex<Option<String>>>,
) -> AudioRef {
    match source {
        AudioRef::RemoteUrl(url) if url.is_empty() => next_remote_audio
            .lock()
            .await
            .take()
            .map(AudioRef::RemoteUrl)
            .unwrap_or(AudioRef::RemoteUrl(url)),
        other => other,
    }
}

async fn fetch_random_question(
    operator: &dyn OperatorClient,
    event_tx: &mpsc::Sender<Event>,
    bus: &TelemetryBus,
    next_remote_audio: &Arc<Mutex<Option<String>>>,
) {
    match retry_operator("GET /v1/questions/random", bus, || {
        operator.random_question()
    })
    .await
    {
        Ok(question) => {
            *next_remote_audio.lock().await = Some(question.audio_url);
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
    next_remote_audio: &Arc<Mutex<Option<String>>>,
) {
    match retry_operator("GET /v1/messages/random", bus, || operator.random_message()).await {
        Ok(message) => {
            *next_remote_audio.lock().await = Some(message.audio_url);
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

async fn upload_recording(
    operator: &dyn OperatorClient,
    audio_source: &dyn AudioSource,
    event_tx: &mpsc::Sender<Event>,
    bus: &TelemetryBus,
    recording_id: RecordingId,
    question_id: booth_hal::QuestionId,
) {
    let result = async {
        let path = audio_source
            .path_of(&recording_id)
            .await
            .map_err(|err| OperatorError::Transport(err.to_string().into()))?;
        let slot = retry_operator("POST /v1/uploads", bus, || {
            operator.init_upload(Some(&question_id))
        })
        .await?;
        retry_operator("PUT <presigned-upload-url>", bus, || {
            operator.put_upload(&slot, &path)
        })
        .await?;
        retry_operator("POST /v1/uploads/{id}/complete", bus, || {
            operator.complete_upload(&slot.id, &recording_id, 0)
        })
        .await?;
        Ok::<(), OperatorError>(())
    }
    .await;

    match result {
        Ok(()) => {
            let _ = event_tx.send(Event::UploadComplete).await;
        }
        Err(err) => {
            publish_operator_error(bus, "upload_recording", &err);
            let _ = event_tx
                .send(Event::UploadFailed {
                    reason: err.to_string(),
                })
                .await;
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

#[cfg(all(feature = "systemd", unix))]
fn send_systemd_notify(message: &str) -> std::io::Result<()> {
    use std::os::unix::net::UnixDatagram;

    let Some(socket) = env::var_os("NOTIFY_SOCKET") else {
        return Ok(());
    };
    let socket = socket.to_string_lossy();
    let target = if let Some(stripped) = socket.strip_prefix('@') {
        let mut abstract_name = String::from("\0");
        abstract_name.push_str(stripped);
        abstract_name
    } else {
        socket.into_owned()
    };
    let datagram = UnixDatagram::unbound()?;
    datagram.send_to(message.as_bytes(), target)?;
    Ok(())
}

#[derive(Default)]
struct MemoryStorage {
    inner: Mutex<HashMap<String, Vec<u8>>>,
}

#[async_trait]
impl Storage for MemoryStorage {
    async fn get(&self, key: &str) -> StdResult<Option<Vec<u8>>, StorageError> {
        Ok(self.inner.lock().await.get(key).cloned())
    }

    async fn set(&self, key: &str, value: &[u8]) -> StdResult<(), StorageError> {
        self.inner
            .lock()
            .await
            .insert(key.to_string(), value.to_vec());
        Ok(())
    }

    async fn delete(&self, key: &str) -> StdResult<(), StorageError> {
        self.inner.lock().await.remove(key);
        Ok(())
    }
}
