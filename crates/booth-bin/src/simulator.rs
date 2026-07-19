//! Interactive TUI that drives the phone runtime through a mocked GPIO port,
//! plus a read-only monitor variant for real hardware.
//!
//! The simulator ([`run_simulator`]) gives developers a way to exercise the
//! full booth pipeline — the state machine, audio playback/capture, and the
//! operator HTTP client — from a development machine that has no rotary phone
//! hardware attached. Hardware events (hook lift, dial pulses) are synthesized
//! by keypresses and injected into a [`booth_mock::MockGpioPort`]. Audio and
//! the operator client are either real (`PiAudioSink`/`PiAudioSource`/
//! `PiOperatorClient`) or mock, depending on whether `--mock` was passed
//! alongside `--simulator`.
//!
//! The monitor ([`run_monitor`]) reuses the same TUI surface but builds the
//! *real* HAL adapters and injects nothing: it is a live, read-only view of
//! the booth as you dial the physical phone, useful for on-Pi bring-up before
//! a USB console/keyboard-driven simulator is available.

#![cfg(feature = "simulator")]

use std::collections::VecDeque;
use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use booth_debug::RuntimeCommand;
use booth_hal::{AudioChannel, GpioEdge, PinRole, TelemetryEvent};
use booth_telemetry::{TelemetryBus, TelemetryRecord};
use crossterm::event::{Event as CtEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::{SinkExt, StreamExt};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap};
use reqwest::StatusCode;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Iso8601;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior, interval};
use tokio_tungstenite::Connector;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::connect_async_tls_with_config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use url::Url;

use crate::{RuntimeConfig, RuntimeOptions, build_simulator_adapters, spawn_runtime};
use booth_hal::RuntimeMode;

const EVENT_HISTORY: usize = 64;
const RENDER_TICK: Duration = Duration::from_millis(100);
const DEFAULT_OPERATOR_URL: &str = "https://operator.example.com";
const ATTACH_CHANNEL_CAPACITY: usize = 256;
const ATTACH_RECONNECT_INITIAL: Duration = Duration::from_millis(500);
const ATTACH_RECONNECT_MAX: Duration = Duration::from_secs(5);
const DEFAULT_DEBUG_ENV_PATH: &str = "/etc/phone-booth/env";

/// Run the simulator TUI to completion.
///
/// `mock_io` selects whether audio and the operator client are mocked or
/// backed by the real `booth-pi` adapters. `log_path` is the file the TUI
/// surfaces in its footer so the user knows where logs were redirected to
/// (set by `install_simulator_tracing` in the `booth-bin` binary).
pub async fn run_simulator(
    config: RuntimeConfig,
    mock_io: bool,
    log_path: Option<String>,
) -> Result<()> {
    let bus = TelemetryBus::new(config.ring_buffer_capacity());

    if !mock_io {
        if config.operator.base_url == DEFAULT_OPERATOR_URL {
            tracing::warn!(
                base_url = %config.operator.base_url,
                "simulator running against the default example operator URL; \
                 operator-driven dial keys will fail. Set [operator].base_url \
                 in your config or pass --mock to use mock adapters."
            );
        }
        if config.operator.token.trim().is_empty() {
            tracing::warn!(
                "simulator running with an empty operator token; \
                 authenticated routes will return 401. Set \
                 PHONE_BOOTH_OPERATOR__TOKEN or pass --mock."
            );
        }
    }

    let (adapters, injector) =
        build_simulator_adapters(&config, &bus, mock_io, RuntimeMode::Simulator)?;

    // Simulator mode is, by definition, the surface where injecting events is
    // the whole point — so light up the embedded debug/web simulator alongside
    // the TUI and pre-enable `allow_controls`. Both surfaces inject through
    // the same `event_tx` and observe the same `TelemetryBus`, so they stay
    // in lock-step automatically. Real (headless) mode is unaffected: it
    // takes a different code path through `main::run` and the
    // `runtime_mode = Real` guard inside `ensure_controls` keeps blocking
    // `/v1/simulate/*` with the "headless" banner.
    let mut runtime_config = config;
    if !runtime_config.debug.allow_controls {
        tracing::info!(
            "simulator mode: enabling [debug] allow_controls so the embedded \
             web simulator can inject events alongside the TUI"
        );
        runtime_config.debug.allow_controls = true;
    }

    // Surface what the web simulator URL will be (or why it won't be
    // reachable) BEFORE the runtime starts. The debug surface logs its own
    // `MissingToken` error at `error!` level if it can't start, but that's
    // easy to miss in the redirected simulator log — and the user has every
    // reason to expect the web UI to work in simulator mode now that the
    // docs say so. Give them an actionable hint either way.
    //
    // The token can come from either the top-level `debug_token` field
    // (which `run_runtime` copies into `debug.token`) or directly from the
    // `[debug] token` setting, so check both before deciding the surface
    // will fail.
    let token_configured =
        runtime_config.debug.token.is_some() || runtime_config.debug_token.is_some();
    if !token_configured && !runtime_config.debug.allow_tokenless {
        tracing::warn!(
            "web simulator disabled: set [debug] token = \"<secret>\" in \
             config (or BOOTH_DEBUG_TOKEN / BOOTH_DEBUG_TOKEN_FILE), or set \
             [debug] allow_tokenless = true for local-only dev, to enable \
             the web UI at http://{}/v1/ui/simulator",
            runtime_config.debug.loopback_bind,
        );
    } else {
        tracing::info!(
            "web simulator: http://{}/v1/ui/simulator",
            runtime_config.debug.loopback_bind,
        );
    }

    let handle = spawn_runtime(
        runtime_config,
        adapters,
        bus.clone(),
        RuntimeOptions {
            start_debug: true,
            listen_signals: false,
            notify_systemd: false,
            runtime_mode: RuntimeMode::Simulator,
        },
    );

    let state = SimulatorState::new(mock_io, false, log_path);
    drive_tui(
        Some(handle),
        TelemetryFeed::local(&bus),
        state,
        Some(injector),
    )
    .await
}

/// Run the read-only hardware monitor TUI to completion.
///
/// Unlike [`run_simulator`], this builds the *real* HAL adapters (or the mock
/// adapters when `mock` is `true`) and never injects synthetic GPIO events —
/// the operator dials the physical rotary phone and the TUI streams the live
/// telemetry (state transitions, decoded digits, audio levels, operator
/// calls) in a scrolling event log.
///
/// It reserves the same GPIO pins and audio device as the packaged
/// `telephone-booth.service`, so the systemd service must be stopped first
/// (`sudo systemctl stop telephone-booth`) to avoid contention. `log_path` is
/// surfaced in the footer and is set by `install_simulator_tracing`.
pub async fn run_monitor(
    config: RuntimeConfig,
    mock: bool,
    log_path: Option<String>,
) -> Result<()> {
    let bus = TelemetryBus::new(config.ring_buffer_capacity());
    let runtime_mode = if mock {
        RuntimeMode::Mock
    } else {
        RuntimeMode::Real
    };

    let adapters = if mock {
        crate::build_mock_adapters(&bus).0
    } else {
        crate::build_pi_adapters(&config, &bus, runtime_mode)?
    };

    let handle = spawn_runtime(
        config,
        adapters,
        bus.clone(),
        RuntimeOptions {
            start_debug: true,
            listen_signals: false,
            notify_systemd: false,
            runtime_mode,
        },
    );

    let state = SimulatorState::new(mock, true, log_path);
    drive_tui(Some(handle), TelemetryFeed::local(&bus), state, None).await
}

/// Attach the read-only TUI to a running booth's debug surface instead of
/// spawning a second local runtime.
pub async fn run_attached(
    config: RuntimeConfig,
    attach_url: Url,
    token: String,
    log_path: Option<String>,
) -> Result<()> {
    validate_attach_url_security(&attach_url)?;
    let target = attach_target_label(&attach_url)?;
    let state = SimulatorState::attached(target.clone(), log_path);
    let feed = TelemetryFeed::remote(AttachConfig {
        base_url: attach_url,
        token,
        loopback_bind: config.debug.loopback_bind.clone(),
        target_label: target,
    });
    drive_tui(None, feed, state, None).await
}

enum TelemetryMessage {
    Record(TelemetryRecord),
    Lagged(u64),
    Status(String),
    Closed,
}

enum TelemetryStream {
    Local(tokio::sync::broadcast::Receiver<TelemetryRecord>),
    Remote(mpsc::Receiver<TelemetryMessage>),
}

struct TelemetryFeed {
    stream: TelemetryStream,
    background_task: Option<tokio::task::JoinHandle<()>>,
}

impl TelemetryFeed {
    fn local(bus: &TelemetryBus) -> Self {
        Self {
            stream: TelemetryStream::Local(bus.subscribe()),
            background_task: None,
        }
    }

    fn remote(config: AttachConfig) -> Self {
        let (tx, rx) = mpsc::channel(ATTACH_CHANNEL_CAPACITY);
        let task = tokio::spawn(async move {
            attach_telemetry_loop(config, tx).await;
        });
        Self {
            stream: TelemetryStream::Remote(rx),
            background_task: Some(task),
        }
    }

    async fn recv(&mut self) -> TelemetryMessage {
        match &mut self.stream {
            TelemetryStream::Local(rx) => match rx.recv().await {
                Ok(record) => TelemetryMessage::Record(record),
                Err(RecvError::Lagged(skipped)) => TelemetryMessage::Lagged(skipped),
                Err(RecvError::Closed) => TelemetryMessage::Closed,
            },
            TelemetryStream::Remote(rx) => rx
                .recv()
                .await
                .map_or(TelemetryMessage::Closed, |message| message),
        }
    }

    fn shutdown(&mut self) {
        if let Some(task) = self.background_task.take() {
            task.abort();
        }
    }
}

#[derive(Clone)]
struct AttachConfig {
    base_url: Url,
    token: String,
    loopback_bind: String,
    target_label: String,
}

#[derive(Debug, serde::Deserialize)]
struct CertFingerprintResponse {
    sha256: String,
}

#[derive(Debug, serde::Serialize)]
struct ReplayRequest {
    replay_from: u64,
}

/// Shared TUI event loop for the interactive simulator and the read-only
/// monitor. When `injector` is `Some`, hook/dial keypresses synthesize GPIO
/// edges; when `None`, the TUI is read-only and only responds to quit keys.
async fn drive_tui(
    handle: Option<crate::RuntimeHandle>,
    mut telemetry: TelemetryFeed,
    mut state: SimulatorState,
    injector: Option<booth_mock::GpioInjector>,
) -> Result<()> {
    let mut terminal = TerminalGuard::enter().context("enter terminal alternate screen")?;
    let mut events = EventStream::new();
    let mut ticker = interval(RENDER_TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    terminal.draw(&state)?;

    let outcome: Result<()> = loop {
        tokio::select! {
            biased;
            key = events.next() => {
                match key {
                    Some(Ok(CtEvent::Key(key))) => {
                        if matches!(state.handle_key(key, injector.as_ref()).await, Action::Quit) {
                            if let Some(handle) = &handle {
                                let _ = handle.commands.send(RuntimeCommand::Shutdown).await;
                            }
                            break Ok(());
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => break Err(err).context("read terminal event"),
                    None => break Ok(()),
                }
            }
            message = telemetry.recv() => {
                match message {
                    TelemetryMessage::Record(record) => state.ingest(&record),
                    TelemetryMessage::Lagged(skipped) => state.note_lag(skipped),
                    TelemetryMessage::Status(status) => state.set_status(status, Style::default()),
                    TelemetryMessage::Closed => break Ok(()),
                }
            }
            _ = ticker.tick() => {}
        }
        terminal.draw(&state)?;
    };

    // Always restore the terminal before printing or returning.
    drop(terminal);

    telemetry.shutdown();

    // Wait briefly for the local runtime to finish cleanly. The runtime task
    // exits when it sees Shutdown above.
    if let Some(handle) = handle {
        match tokio::time::timeout(Duration::from_secs(2), handle.join).await {
            Ok(Ok(Ok(final_state))) => {
                tracing::info!(state = final_state.tag(), "tui runtime stopped");
            }
            Ok(Ok(Err(err))) => tracing::warn!(error = %err, "runtime exited with error"),
            Ok(Err(join_err)) => tracing::warn!(error = %join_err, "runtime task panicked"),
            Err(_) => tracing::warn!("runtime did not stop within 2s of shutdown"),
        }
    }

    outcome
}

/// Resolve the debug bearer token used by attach mode before the TUI takes over
/// stderr/stdout.
pub fn resolve_attach_token(config: &RuntimeConfig, cli_token: Option<String>) -> Result<String> {
    if let Some(token) = cli_token {
        return Ok(token);
    }
    if let Some(token) = config.debug_token.clone() {
        return Ok(token);
    }
    if let Some(token) = config.debug.token.as_ref().map(|token| token.0.clone()) {
        return Ok(token);
    }
    if let Some(token) = read_debug_token_from_env_file(DEFAULT_DEBUG_ENV_PATH)? {
        return Ok(token);
    }
    bail!(
        "attach mode requires a debug bearer token; pass --token, set BOOTH_DEBUG_TOKEN \
         or BOOTH_DEBUG_TOKEN_FILE, or add BOOTH_DEBUG_TOKEN to {DEFAULT_DEBUG_ENV_PATH}"
    );
}

fn read_debug_token_from_env_file(path: &str) -> Result<Option<String>> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("read {path}")),
    };

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "BOOTH_DEBUG_TOKEN" {
            return Ok(Some(strip_optional_quotes(value.trim()).to_string()));
        }
    }

    Ok(None)
}

fn strip_optional_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn attach_target_label(url: &Url) -> Result<String> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("attach URL must include a host"))?;
    Ok(url
        .port()
        .map_or_else(|| host.to_string(), |port| format!("{host}:{port}")))
}

fn is_loopback_host(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost"))
        || url
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback())
}

fn validate_attach_url_security(base_url: &Url) -> Result<()> {
    match base_url.scheme() {
        "http" | "ws" if !is_loopback_host(base_url) => bail!(
            "plaintext attach URLs are only allowed for loopback hosts; use https:// or wss://"
        ),
        "http" | "https" | "ws" | "wss" => Ok(()),
        other => bail!(
            "unsupported attach URL scheme `{other}`; use http:// or ws:// for loopback, \
             https:// or wss:// for remote hosts"
        ),
    }
}

fn websocket_url(base_url: &Url) -> Result<Url> {
    validate_attach_url_security(base_url)?;
    let mut url = base_url.clone();
    match url.scheme() {
        "http" => {
            url.set_scheme("ws")
                .map_err(|()| anyhow!("convert attach URL scheme to ws"))?;
        }
        "https" => {
            url.set_scheme("wss")
                .map_err(|()| anyhow!("convert attach URL scheme to wss"))?;
        }
        "ws" | "wss" => {}
        other => bail!(
            "unsupported attach URL scheme `{other}`; use http://, https://, ws://, or wss://"
        ),
    }
    url.set_path("/v1/ws/telemetry");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

async fn attach_telemetry_loop(config: AttachConfig, tx: mpsc::Sender<TelemetryMessage>) {
    let mut replay_from = Some(0_u64);
    let mut backoff = ATTACH_RECONNECT_INITIAL;
    let target = config.target_label.clone();

    let _ = tx
        .send(TelemetryMessage::Status(format!("Connecting to {target}…")))
        .await;

    'reconnect: loop {
        match connect_attach_socket(&config).await {
            Ok(mut socket) => {
                backoff = ATTACH_RECONNECT_INITIAL;
                let _ = tx
                    .send(TelemetryMessage::Status(format!(
                        "Attached to {target} (read-only telemetry stream)."
                    )))
                    .await;

                if let Some(last_seen) = replay_from {
                    let replay_request = ReplayRequest {
                        replay_from: last_seen,
                    };
                    let replay = match serde_json::to_string(&replay_request) {
                        Ok(replay) => replay,
                        Err(err) => {
                            let _ = tx
                                .send(TelemetryMessage::Status(format!(
                                    "Attach replay request failed; reconnecting… ({err})"
                                )))
                                .await;
                            tokio::time::sleep(backoff).await;
                            backoff = (backoff * 2).min(ATTACH_RECONNECT_MAX);
                            continue 'reconnect;
                        }
                    };
                    if let Err(err) = socket.send(Message::Text(replay)).await {
                        let _ = tx
                            .send(TelemetryMessage::Status(format!(
                                "Attach replay request failed; reconnecting… ({err})"
                            )))
                            .await;
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(ATTACH_RECONNECT_MAX);
                        continue;
                    }
                }

                loop {
                    match socket.next().await {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<TelemetryRecord>(&text) {
                                Ok(record) => {
                                    replay_from = Some(record.id);
                                    if tx.send(TelemetryMessage::Record(record)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(err) => {
                                    let _ = tx
                                        .send(TelemetryMessage::Status(format!(
                                            "Invalid telemetry frame; reconnecting… ({err})"
                                        )))
                                        .await;
                                    break;
                                }
                            }
                        }
                        Some(Ok(Message::Binary(bytes))) => {
                            match serde_json::from_slice::<TelemetryRecord>(&bytes) {
                                Ok(record) => {
                                    replay_from = Some(record.id);
                                    if tx.send(TelemetryMessage::Record(record)).await.is_err() {
                                        return;
                                    }
                                }
                                Err(err) => {
                                    let _ = tx
                                        .send(TelemetryMessage::Status(format!(
                                            "Invalid telemetry frame; reconnecting… ({err})"
                                        )))
                                        .await;
                                    break;
                                }
                            }
                        }
                        Some(Ok(Message::Close(_))) => break,
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                        Some(Ok(Message::Frame(_))) => {}
                        Some(Err(err)) => {
                            let _ = tx
                                .send(TelemetryMessage::Status(format!(
                                    "Attach stream dropped; reconnecting… ({err})"
                                )))
                                .await;
                            break;
                        }
                        None => break,
                    }
                }
            }
            Err(err) => {
                let _ = tx
                    .send(TelemetryMessage::Status(format!(
                        "Attach failed; reconnecting in {} ms ({err})",
                        backoff.as_millis()
                    )))
                    .await;
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(ATTACH_RECONNECT_MAX);
    }
}

async fn connect_attach_socket(
    config: &AttachConfig,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
> {
    let ws_url = websocket_url(&config.base_url)?;
    let mut request = ws_url.as_str().into_client_request()?;
    let mut authorization = String::from("Bearer ");
    authorization.push_str(&config.token);
    request.headers_mut().insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&authorization)
            .context("build attach Authorization header")?,
    );

    let is_pinned_loopback =
        matches!(config.base_url.scheme(), "https" | "wss") && is_loopback_host(&config.base_url);
    if is_pinned_loopback {
        let fingerprint = fetch_loopback_fingerprint(config).await?;
        let connector = build_fingerprint_connector(&fingerprint)?;
        let (socket, _response) =
            connect_async_tls_with_config(request, None, false, Some(connector)).await?;
        Ok(socket)
    } else {
        let (socket, _response) = connect_async(request).await?;
        Ok(socket)
    }
}

async fn fetch_loopback_fingerprint(config: &AttachConfig) -> Result<String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .context("build fingerprint client")?;
    let response = client
        .get(format!(
            "http://{}/v1/cert/fingerprint",
            config.loopback_bind
        ))
        .bearer_auth(&config.token)
        .send()
        .await
        .context("fetch loopback certificate fingerprint")?;
    if response.status() != StatusCode::OK {
        bail!(
            "fetch loopback certificate fingerprint failed with HTTP {}; \
             ensure the running service still exposes the loopback debug listener on {}",
            response.status(),
            config.loopback_bind
        );
    }
    let body: CertFingerprintResponse = response
        .json()
        .await
        .context("decode loopback certificate fingerprint response")?;
    Ok(body.sha256)
}

fn build_fingerprint_connector(expected_fingerprint: &str) -> Result<Connector> {
    let verifier = WebPkiServerVerifier::builder(Arc::new(rustls::RootCertStore::empty()))
        .build()
        .context("build TLS signature verifier")?;
    let config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(FingerprintVerifier {
            expected_sha256: expected_fingerprint.to_ascii_lowercase(),
            signature_verifier: verifier,
        }))
        .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(config)))
}

#[derive(Debug)]
struct FingerprintVerifier {
    expected_sha256: String,
    signature_verifier: Arc<WebPkiServerVerifier>,
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let digest = Sha256::digest(end_entity.as_ref());
        let actual = format_sha256_hex(digest.as_slice());
        if actual == self.expected_sha256 {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "debug TLS certificate fingerprint did not match".to_string(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.signature_verifier
            .verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.signature_verifier
            .verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.signature_verifier.supported_verify_schemes()
    }
}

fn format_sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

// ---------------------------------------------------------------------------
// Terminal guard: ensures raw mode + alternate screen are restored even on
// panic, before any error is printed to stderr.
// ---------------------------------------------------------------------------

struct TerminalGuard {
    terminal: Option<Terminal<CrosstermBackend<Stdout>>>,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend).context("create terminal")?;
        terminal.clear().context("clear terminal")?;
        Ok(Self {
            terminal: Some(terminal),
        })
    }

    fn draw(&mut self, state: &SimulatorState) -> Result<()> {
        let Some(terminal) = self.terminal.as_mut() else {
            return Ok(());
        };
        terminal
            .draw(|frame| state.render(frame))
            .context("draw terminal frame")?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Some(mut terminal) = self.terminal.take() {
            let _ = disable_raw_mode();
            let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
            let _ = terminal.show_cursor();
        }
    }
}

// ---------------------------------------------------------------------------
// Simulator state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Action {
    Continue,
    Quit,
}

struct SimulatorState {
    mock_io: bool,
    /// When `true`, the TUI is a read-only monitor over real hardware: hook and
    /// dial keypresses are ignored (there is no injector) and the header/footer
    /// reflect a live-hardware view instead of the interactive simulator.
    read_only: bool,
    attached_to: Option<String>,
    log_path: Option<String>,
    hook_on_hook: bool,
    /// Whether we have observed the physical hook position yet. The monitor
    /// starts without knowing it (`PiGpioPort` emits only interrupts, so there
    /// is no initial snapshot); the header shows "unknown" until the first
    /// hook edge arrives. The interactive simulator always knows it (it starts
    /// the mocked dial on-hook).
    hook_known: bool,
    current_state: String,
    booth_status: String,
    audio_in: LevelView,
    audio_out: LevelView,
    history: VecDeque<HistoryEntry>,
    status_line: String,
    lagged_records: u64,
    started_at: Instant,
}

#[derive(Default, Clone, Copy)]
struct LevelView {
    peak: f32,
    rms: f32,
}

struct HistoryEntry {
    ts: OffsetDateTime,
    text: String,
    style: Style,
}

impl SimulatorState {
    fn new(mock_io: bool, read_only: bool, log_path: Option<String>) -> Self {
        let status_line = if read_only {
            "Live hardware monitor — dial the real phone to see events.".to_string()
        } else {
            "Press [h] or space to lift the receiver.".to_string()
        };
        Self {
            mock_io,
            read_only,
            attached_to: None,
            log_path,
            hook_on_hook: true,
            hook_known: !read_only,
            current_state: "idle".to_string(),
            booth_status: "idle".to_string(),
            audio_in: LevelView::default(),
            audio_out: LevelView::default(),
            history: VecDeque::with_capacity(EVENT_HISTORY),
            status_line,
            lagged_records: 0,
            started_at: Instant::now(),
        }
    }

    fn attached(target: String, log_path: Option<String>) -> Self {
        let status_line = format!("Connecting to {target}…");
        Self {
            mock_io: false,
            read_only: true,
            attached_to: Some(target),
            log_path,
            hook_on_hook: true,
            hook_known: false,
            current_state: "idle".to_string(),
            booth_status: "idle".to_string(),
            audio_in: LevelView::default(),
            audio_out: LevelView::default(),
            history: VecDeque::with_capacity(EVENT_HISTORY),
            status_line,
            lagged_records: 0,
            started_at: Instant::now(),
        }
    }

    async fn handle_key(
        &mut self,
        key: KeyEvent,
        injector: Option<&booth_mock::GpioInjector>,
    ) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            KeyCode::Char('c') if ctrl => return Action::Quit,
            KeyCode::Char('h' | ' ') => {
                if let Some(injector) = injector {
                    self.toggle_hook(injector).await;
                } else {
                    self.note_read_only();
                }
            }
            KeyCode::Char(c @ '0'..='9') => {
                if let Some(injector) = injector {
                    if self.hook_on_hook {
                        self.set_status(
                            "Lift the receiver before dialing (press [h] or space).",
                            Style::default().fg(Color::Yellow),
                        );
                    } else if let Some(digit) = c.to_digit(10).and_then(|d| u8::try_from(d).ok()) {
                        self.dial_digit(digit, injector).await;
                    }
                } else {
                    self.note_read_only();
                }
            }
            _ => {}
        }
        Action::Continue
    }

    fn note_read_only(&mut self) {
        self.set_status(
            if self.attached_to.is_some() {
                "Attach mode is read-only — watch the running booth or use the web simulator controls."
            } else {
                "Live hardware monitor — use the real phone (input is read-only)."
            },
            Style::default().fg(Color::Yellow),
        );
    }

    async fn toggle_hook(&mut self, injector: &booth_mock::GpioInjector) {
        self.hook_on_hook = !self.hook_on_hook;
        let level = self.hook_on_hook;
        injector
            .push(GpioEdge {
                role: PinRole::Hook,
                level,
                at_monotonic_ns: self.monotonic_ns(),
            })
            .await;
        let action = if level { "Hung up" } else { "Lifted receiver" };
        self.set_status(action.to_string(), Style::default().fg(Color::Cyan));
        self.push_history(
            format!("inject: hook level={level} ({action})"),
            Style::default().fg(Color::Cyan),
        );
    }

    async fn dial_digit(&mut self, digit: u8, injector: &booth_mock::GpioInjector) {
        // A rotary "0" sends 10 pulses; otherwise one pulse per unit.
        let pulses = if digit == 0 { 10 } else { digit };
        for _ in 0..pulses {
            // Inject both falling + rising edges so the injected stream
            // matches what a real rotary dial produces, even though
            // event_from_gpio only forwards the falling edge into the core.
            injector
                .push(GpioEdge {
                    role: PinRole::RotaryPulse,
                    level: false,
                    at_monotonic_ns: self.monotonic_ns(),
                })
                .await;
            injector
                .push(GpioEdge {
                    role: PinRole::RotaryPulse,
                    level: true,
                    at_monotonic_ns: self.monotonic_ns(),
                })
                .await;
        }
        self.set_status(
            format!("Dialed {digit} ({pulses} pulses)"),
            Style::default().fg(Color::Cyan),
        );
        self.push_history(
            format!("inject: dial {digit} ({pulses} pulses)"),
            Style::default().fg(Color::Cyan),
        );
    }

    fn ingest(&mut self, record: &TelemetryRecord) {
        let ts = OffsetDateTime::from(record.ts);
        match &record.event {
            TelemetryEvent::StateTransition {
                from: _, to, cause, ..
            } => {
                self.current_state = to.clone();
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("state -> {to} (cause: {cause})"),
                    style: Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                });
            }
            TelemetryEvent::DigitDialed { digit, pulses, .. } => {
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("digit decoded: {digit} ({pulses} pulses)"),
                    style: Style::default().fg(Color::Magenta),
                });
            }
            TelemetryEvent::AudioLevel(level) => {
                let view = LevelView {
                    peak: level.peak,
                    rms: level.rms,
                };
                match level.channel {
                    AudioChannel::Input => self.audio_in = view,
                    AudioChannel::Output => self.audio_out = view,
                }
            }
            TelemetryEvent::AudioDeviceChange { name, channel } => {
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("audio device ({channel:?}): {name}"),
                    style: Style::default().fg(Color::Blue),
                });
            }
            TelemetryEvent::OperatorRequest { id, route } => {
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("operator -> {route} (id={id})"),
                    style: Style::default().fg(Color::Blue),
                });
            }
            TelemetryEvent::OperatorResponse {
                id,
                status,
                duration_ms,
            } => {
                let color = if *status >= 400 {
                    Color::Red
                } else {
                    Color::Blue
                };
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("operator <- {status} in {duration_ms}ms (id={id})"),
                    style: Style::default().fg(color),
                });
            }
            TelemetryEvent::Error { source, message } => {
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("error [{source}] {message}"),
                    style: Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                });
            }
            TelemetryEvent::Log {
                level,
                target,
                message,
            } => {
                let color = match level.as_str() {
                    "error" => Color::Red,
                    "warn" => Color::Yellow,
                    _ => Color::DarkGray,
                };
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("{level} [{target}] {message}"),
                    style: Style::default().fg(color),
                });
            }
            TelemetryEvent::GpioEdge(edge) => {
                // Track hook position from real hardware edges so the read-only
                // monitor's header reflects the physical receiver. Per the
                // active-low pull-up wiring (docs/hardware.md), a high level on
                // the hook pin means the receiver is resting (on-hook).
                if edge.role == PinRole::Hook {
                    self.hook_on_hook = edge.level;
                    self.hook_known = true;
                }
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("gpio edge {:?} level={}", edge.role, edge.level),
                    style: Style::default().fg(Color::DarkGray),
                });
            }
            TelemetryEvent::SystemSample { .. } => {
                // The simulator does not currently render the live system
                // panel; the operator UI is the authoritative surface.
            }
            TelemetryEvent::CallStarted { session_id, .. } => {
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("call started (session={session_id})"),
                    style: Style::default().fg(Color::Green),
                });
            }
            TelemetryEvent::CallEnded {
                session_id,
                outcome,
                ..
            } => {
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("call ended ({outcome}, session={session_id})"),
                    style: Style::default().fg(Color::Green),
                });
            }
            TelemetryEvent::RecordingStarted { id, session_id, .. } => {
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("recording started id={id} session={session_id}"),
                    style: Style::default().fg(Color::Magenta),
                });
            }
            TelemetryEvent::RecordingStopped {
                id,
                duration_ms,
                bytes,
                ..
            } => {
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!(
                        "recording stopped id={id} duration={duration_ms}ms bytes={bytes}"
                    ),
                    style: Style::default().fg(Color::Magenta),
                });
            }
            TelemetryEvent::UploadStarted { recording_id, .. } => {
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("upload started recording={recording_id}"),
                    style: Style::default().fg(Color::Blue),
                });
            }
            TelemetryEvent::UploadCompleted {
                recording_id,
                duration_ms,
                bytes,
                ..
            } => {
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!(
                        "upload completed recording={recording_id} duration={duration_ms}ms bytes={bytes}"
                    ),
                    style: Style::default().fg(Color::Blue),
                });
            }
            TelemetryEvent::UploadFailed {
                recording_id,
                message,
                ..
            } => {
                self.history.push_front(HistoryEntry {
                    ts,
                    text: format!("upload failed recording={recording_id}: {message}"),
                    style: Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                });
            }
        }
        self.booth_status = derive_booth_status(&self.current_state).to_string();
        while self.history.len() > EVENT_HISTORY {
            self.history.pop_back();
        }
    }

    fn note_lag(&mut self, skipped: u64) {
        self.lagged_records = self.lagged_records.saturating_add(skipped);
        self.set_status(
            format!("Telemetry lag: dropped {skipped} records"),
            Style::default().fg(Color::Yellow),
        );
    }

    fn set_status<S: Into<String>>(&mut self, text: S, _style: Style) {
        // Style currently unused; kept so we can colorize the footer later.
        self.status_line = text.into();
    }

    fn push_history(&mut self, text: String, style: Style) {
        self.history.push_front(HistoryEntry {
            ts: OffsetDateTime::now_utc(),
            text,
            style,
        });
        while self.history.len() > EVENT_HISTORY {
            self.history.pop_back();
        }
    }

    fn monotonic_ns(&self) -> u64 {
        let elapsed = self.started_at.elapsed();
        u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
    }

    fn render(&self, frame: &mut ratatui::Frame<'_>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // header
                Constraint::Min(6),    // history
                Constraint::Length(4), // audio meters
                Constraint::Length(3), // footer / controls
            ])
            .split(frame.area());

        self.render_header(frame, chunks[0]);
        self.render_history(frame, chunks[1]);
        self.render_audio(frame, chunks[2]);
        self.render_footer(frame, chunks[3]);
    }

    fn render_header(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let mode = if self.mock_io { "mock I/O" } else { "real I/O" };
        let title = if self.read_only {
            "Telephone Booth Monitor"
        } else {
            "Telephone Booth Simulator"
        };
        let mode_badge = self.attached_to.as_ref().map_or_else(
            || format!("[{mode}]"),
            |target| format!("[attached: {target}]"),
        );
        let hook = if !self.hook_known {
            "unknown"
        } else if self.hook_on_hook {
            "on-hook"
        } else {
            "off-hook"
        };
        let header = Line::from(vec![
            Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("   "),
            Span::styled(mode_badge, Style::default().fg(Color::DarkGray)),
            Span::raw("   state="),
            Span::styled(
                self.current_state.clone(),
                Style::default().fg(Color::Green),
            ),
            Span::raw("   status="),
            Span::styled(self.booth_status.clone(), Style::default().fg(Color::Green)),
            Span::raw("   hook="),
            Span::styled(
                hook,
                Style::default().fg(if !self.hook_known {
                    Color::DarkGray
                } else if self.hook_on_hook {
                    Color::Yellow
                } else {
                    Color::Cyan
                }),
            ),
        ]);
        let para = Paragraph::new(header)
            .block(Block::default().borders(Borders::ALL).title(" Booth "))
            .wrap(Wrap { trim: true });
        frame.render_widget(para, area);
    }

    fn render_history(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let items: Vec<ListItem<'_>> = self
            .history
            .iter()
            .take(area.height.saturating_sub(2) as usize)
            .map(|entry| {
                let ts = entry
                    .ts
                    .format(&Iso8601::DEFAULT)
                    .unwrap_or_else(|_| "????-??-??T??:??:??".to_string());
                let ts = ts.split('.').next().unwrap_or(&ts).to_string();
                ListItem::new(Line::from(vec![
                    Span::styled(ts, Style::default().fg(Color::DarkGray)),
                    Span::raw("  "),
                    Span::styled(entry.text.clone(), entry.style),
                ]))
            })
            .collect();
        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Events (newest first) "),
        );
        frame.render_widget(list, area);
    }

    fn render_audio(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);
        let input = build_level_gauge("Audio In", self.audio_in);
        let output = build_level_gauge("Audio Out", self.audio_out);
        frame.render_widget(input, cols[0]);
        frame.render_widget(output, cols[1]);
    }

    fn render_footer(&self, frame: &mut ratatui::Frame<'_>, area: Rect) {
        let controls = if self.attached_to.is_some() {
            "Controls: [q]/Esc/Ctrl+C quit   (attached read-only view)"
        } else if self.read_only {
            "Controls: [q]/Esc/Ctrl+C quit   (live hardware — dial the real phone)"
        } else {
            "Controls: [h]/space toggle hook   [0-9] dial digit   [q]/Esc/Ctrl+C quit"
        };
        let log_line = self.log_path.as_ref().map_or_else(
            || "Log: <stdout>".to_string(),
            |path| format!("Log: {path}"),
        );
        let lag_note = if self.lagged_records > 0 {
            format!("   (dropped {} telemetry records)", self.lagged_records)
        } else {
            String::new()
        };
        let status_line = &self.status_line;
        let text = vec![
            Line::from(controls),
            Line::from(format!("{status_line}  {log_line}{lag_note}")),
        ];
        let para = Paragraph::new(text).block(Block::default().borders(Borders::ALL));
        frame.render_widget(para, area);
    }
}

fn build_level_gauge(title: &str, level: LevelView) -> Gauge<'_> {
    let peak = level.peak.clamp(0.0, 1.0);
    let rms = level.rms.clamp(0.0, 1.0);
    let label = format!("peak {peak:>5.2}   rms {rms:>5.2}");
    let ratio = f64::from(peak);
    Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} ")),
        )
        .gauge_style(Style::default().fg(Color::LightGreen))
        .ratio(ratio.clamp(0.0, 1.0))
        .label(label)
}

fn derive_booth_status(state: &str) -> &'static str {
    match state {
        "idle" | "error" => "idle",
        "dial_tone" | "dialing" => "dial_tone",
        "playing_question" | "beep" => "playing_question",
        "recording" => "recording",
        "uploading" => "uploading",
        "playing_message" => "playing_message",
        "playing_instructions" => "playing_instructions",
        _ => "unknown",
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "tests may panic on setup failure"
)]
mod tests {
    use super::*;
    use booth_debug::{DebugConfig, DebugToken, RuntimeCommand, serve_with_handles};
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn attach_feed_replays_and_streams_live_telemetry() -> Result<()> {
        let config = DebugConfig {
            loopback_bind: "127.0.0.1:0".to_string(),
            lan_enabled: false,
            tailscale_enabled: true,
            allow_tokenless: false,
            token: Some(DebugToken("attach-test-token".to_string())),
            ring_buffer_capacity: 16,
            ..DebugConfig::default()
        };

        let bus = TelemetryBus::new(config.ring_buffer_capacity);
        let (runtime_tx, _runtime_rx) = mpsc::channel::<RuntimeCommand>(4);
        let handles = serve_with_handles(config, bus.clone(), runtime_tx, None).await?;
        let addr = handles
            .loopback_addr
            .expect("loopback listener should be running");

        bus.publish(TelemetryEvent::Log {
            level: "info".to_string(),
            target: "attach-test".to_string(),
            message: "replay-1".to_string(),
        });
        bus.publish(TelemetryEvent::Log {
            level: "info".to_string(),
            target: "attach-test".to_string(),
            message: "replay-2".to_string(),
        });

        let mut feed = TelemetryFeed::remote(AttachConfig {
            base_url: Url::parse(&format!("http://{addr}"))?,
            token: "attach-test-token".to_string(),
            loopback_bind: addr.to_string(),
            target_label: addr.to_string(),
        });

        let mut messages = Vec::new();
        while messages.len() < 2 {
            match tokio::time::timeout(Duration::from_secs(2), feed.recv()).await? {
                TelemetryMessage::Record(record) => {
                    if let TelemetryEvent::Log { message, .. } = record.event {
                        messages.push(message);
                    }
                }
                TelemetryMessage::Status(_) | TelemetryMessage::Lagged(_) => {}
                TelemetryMessage::Closed => bail!("attach feed closed before replay arrived"),
            }
        }

        bus.publish(TelemetryEvent::Log {
            level: "info".to_string(),
            target: "attach-test".to_string(),
            message: "live-3".to_string(),
        });

        loop {
            match tokio::time::timeout(Duration::from_secs(2), feed.recv()).await? {
                TelemetryMessage::Record(record) => {
                    if let TelemetryEvent::Log { message, .. } = record.event {
                        messages.push(message);
                        if messages.len() == 3 {
                            break;
                        }
                    }
                }
                TelemetryMessage::Status(_) | TelemetryMessage::Lagged(_) => {}
                TelemetryMessage::Closed => bail!("attach feed closed before live event arrived"),
            }
        }

        assert_eq!(messages, vec!["replay-1", "replay-2", "live-3"]);

        feed.shutdown();
        let _ = handles.shutdown_tx.send(());
        let _ = handles.handle.await;
        Ok(())
    }

    #[test]
    fn env_file_reader_extracts_debug_token() -> Result<()> {
        let dir = std::env::current_dir()?
            .join("target")
            .join("booth-bin-simulator-tests");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("env-{}", std::process::id()));
        std::fs::write(
            &path,
            "# comment\nINVALID LINE\nOTHER=value\nBOOTH_DEBUG_TOKEN='single-quoted'\n",
        )?;

        let token = read_debug_token_from_env_file(&path.display().to_string())?;
        assert_eq!(token.as_deref(), Some("single-quoted"));

        std::fs::write(&path, "BOOTH_DEBUG_TOKEN=plain-token-with-$pecial_chars\n")?;
        let token = read_debug_token_from_env_file(&path.display().to_string())?;
        assert_eq!(token.as_deref(), Some("plain-token-with-$pecial_chars"));

        std::fs::write(&path, "BOOTH_DEBUG_TOKEN=\n")?;
        let token = read_debug_token_from_env_file(&path.display().to_string())?;
        assert_eq!(token.as_deref(), Some(""));
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn attach_url_allows_plaintext_only_on_loopback() -> Result<()> {
        let http_loopback = Url::parse("http://127.0.0.1:8080/debug?ignored=true")?;
        assert_eq!(
            websocket_url(&http_loopback)?.as_str(),
            "ws://127.0.0.1:8080/v1/ws/telemetry"
        );

        let ws_loopback = Url::parse("ws://localhost:8080/custom")?;
        assert_eq!(
            websocket_url(&ws_loopback)?.as_str(),
            "ws://localhost:8080/v1/ws/telemetry"
        );

        let secure_remote_url = Url::parse("https://telephone-booth.example.com")?;
        assert_eq!(
            websocket_url(&secure_remote_url)?.as_str(),
            "wss://telephone-booth.example.com/v1/ws/telemetry"
        );

        let cleartext_http_url = Url::parse("http://telephone-booth.example.com")?;
        let err = websocket_url(&cleartext_http_url).expect_err("remote plaintext should fail");
        assert!(err.to_string().contains("plaintext attach URLs"));

        let cleartext_ws_url = Url::parse("ws://telephone-booth.example.com")?;
        let err = websocket_url(&cleartext_ws_url).expect_err("remote plaintext should fail");
        assert!(err.to_string().contains("plaintext attach URLs"));

        Ok(())
    }
}
