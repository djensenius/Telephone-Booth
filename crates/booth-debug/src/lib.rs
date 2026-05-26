//! Embedded debug surface for the Telephone Booth phone client.
//!
//! This crate runs a small axum HTTP and WebSocket server inside the booth
//! process. It intentionally depends only on `booth-core`, `booth-hal`, and
//! `booth-telemetry` so the Pi adapter remains below the runtime wiring in the
//! dependency graph. The effective Pi configuration returned from `/v1/config`
//! is therefore represented as a redacted, stable JSON projection instead of a
//! direct `booth-pi::PiConfig` dependency.
//!
//! The server binds two transports when enabled: plaintext loopback HTTP for
//! `tailscale serve` (`127.0.0.1:8080`) and self-signed TLS on the LAN fallback
//! (defaults to `127.0.0.1:8443`, configurable to `0.0.0.0:8443` with a strong
//! token). The LAN listener is disabled by default and requires explicit opt-in.
//! The LAN certificate is generated at process startup and its SHA-256
//! fingerprint is available to loopback clients for operator-side pinning.

#![warn(missing_docs)]

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use axum::extract::connect_info::Connected;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, Query, State, WebSocketUpgrade};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, HeaderName};
use axum::http::{HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use booth_core::Event;
use booth_hal::{
    AudioChannel, AudioLevel, BoothStatus as HalBoothStatus, GpioEdge, PinRole, SystemSnapshot,
    TelemetryEvent,
};
use futures_util::FutureExt;
use parking_lot::Mutex;
use rcgen::CertifiedKey;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tower_http::cors::CorsLayer;
use tracing::field::{Field, Visit};
use tracing::{Event as TracingEvent, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

pub use booth_telemetry::{TelemetryBus, TelemetryRecord};

const DEFAULT_STATE_QUERY_TIMEOUT: Duration = Duration::from_millis(100);
const WS_REPLAY_WAIT: Duration = Duration::from_millis(25);

/// Bearer debug token supplied by the runtime configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugToken(pub String);

/// Configuration for the debug surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugConfig {
    /// Bind address for the loopback listener proxied by `tailscale serve`.
    /// Defaults to `127.0.0.1:8080`.
    #[serde(default = "default_loopback")]
    pub loopback_bind: String,
    /// Bind address for the LAN-fallback TLS listener. Defaults to
    /// `127.0.0.1:8443`.
    #[serde(default = "default_lan")]
    pub lan_bind: String,
    /// Whether to expose the loopback endpoint for `tailscale serve`.
    #[serde(default = "default_true")]
    pub tailscale_enabled: bool,
    /// Whether to expose the LAN-fallback HTTPS endpoint. Defaults to
    /// disabled; operators must opt in explicitly.
    #[serde(default)]
    pub lan_enabled: bool,
    /// Whether `POST /v1/simulate/*` control endpoints are available.
    #[serde(default)]
    pub allow_controls: bool,
    /// Maximum number of telemetry events and log lines retained for catch-up.
    #[serde(default = "default_ring")]
    pub ring_buffer_capacity: usize,
    /// Debug bearer token required for HTTP and WebSocket requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<DebugToken>,
    /// Permit loopback clients to skip bearer auth. Defaults to false.
    #[serde(default)]
    pub loopback_skip_auth: bool,
    /// Allow the debug surface to start without a bearer token. Defaults to
    /// `false` (fail closed). Set to `true` only for local development or
    /// testing where network exposure is not a concern.
    #[serde(default)]
    pub allow_tokenless: bool,
    /// Operator UI origin allowed by CORS, for example `https://operator.example.com`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator_origin: Option<String>,
    /// Effective runtime configuration with secrets already redacted.
    #[serde(default)]
    pub effective_config: ConfigRedacted,
}

fn default_loopback() -> String {
    "127.0.0.1:8080".into()
}

fn default_lan() -> String {
    "127.0.0.1:8443".into()
}

fn default_true() -> bool {
    true
}

fn default_ring() -> usize {
    4096
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            loopback_bind: default_loopback(),
            lan_bind: default_lan(),
            tailscale_enabled: true,
            lan_enabled: false,
            allow_controls: false,
            ring_buffer_capacity: default_ring(),
            token: None,
            loopback_skip_auth: false,
            allow_tokenless: false,
            operator_origin: None,
            effective_config: ConfigRedacted::default(),
        }
    }
}

/// Redacted effective configuration returned by `/v1/config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigRedacted {
    /// GPIO configuration projection, usually the serialized Pi GPIO config.
    pub gpio: serde_json::Value,
    /// Audio configuration projection, usually the serialized Pi audio config.
    pub audio: serde_json::Value,
    /// Operator connection settings with the API token redacted.
    pub operator: OperatorConfigRedacted,
    /// Debug surface configuration safe to expose to an authenticated operator.
    pub debug: DebugConfigRedacted,
    /// Additional future-safe configuration sections.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Default for ConfigRedacted {
    fn default() -> Self {
        Self {
            gpio: serde_json::json!({}),
            audio: serde_json::json!({}),
            operator: OperatorConfigRedacted::default(),
            debug: DebugConfigRedacted::default(),
            extra: BTreeMap::new(),
        }
    }
}

/// Operator configuration projection with secrets redacted.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorConfigRedacted {
    /// Operator base URL, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Status topic or booth identifier, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_topic: Option<String>,
    /// API token redaction, empty when no token is configured.
    #[serde(default)]
    pub token: String,
    /// Additional non-secret operator settings.
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Debug configuration projection returned by `/v1/config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugConfigRedacted {
    /// Whether the loopback listener is enabled.
    pub tailscale_enabled: bool,
    /// Whether the LAN TLS listener is enabled.
    pub lan_enabled: bool,
    /// Whether simulation controls are enabled.
    pub allow_controls: bool,
    /// Replay/log ring buffer capacity.
    pub ring_buffer_capacity: usize,
    /// Operator CORS origin, when configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operator_origin: Option<String>,
    /// Whether loopback auth skipping is enabled.
    pub loopback_skip_auth: bool,
}

impl Default for DebugConfigRedacted {
    fn default() -> Self {
        Self {
            tailscale_enabled: true,
            lan_enabled: false,
            allow_controls: false,
            ring_buffer_capacity: default_ring(),
            operator_origin: None,
            loopback_skip_auth: false,
        }
    }
}

impl DebugConfigRedacted {
    /// Build a safe projection from full debug config.
    #[must_use]
    pub fn from_debug_config(config: &DebugConfig) -> Self {
        Self {
            tailscale_enabled: config.tailscale_enabled,
            lan_enabled: config.lan_enabled,
            allow_controls: config.allow_controls,
            ring_buffer_capacity: config.ring_buffer_capacity,
            operator_origin: config.operator_origin.clone(),
            loopback_skip_auth: config.loopback_skip_auth,
        }
    }
}

/// Return a token redaction that preserves only the final four characters.
#[must_use]
pub fn redact_token(token: &str) -> String {
    if token.is_empty() {
        return "<empty>".to_string();
    }

    let mut last_four = token.chars().rev().take(4).collect::<Vec<_>>();
    last_four.reverse();
    format!("<redacted:{}>", last_four.into_iter().collect::<String>())
}

/// Errors the debug surface can return at startup.
#[derive(Debug, thiserror::Error)]
pub enum DebugError {
    /// Could not bind the requested socket.
    #[error("bind failed: {0}")]
    Bind(String),
    /// TLS certificate or key generation/load failed.
    #[error("tls error: {0}")]
    Tls(String),
    /// LAN listener configured with a non-loopback bind but missing or weak token.
    #[error("insecure lan bind: {0}")]
    InsecureLanBind(String),
    /// Debug listener enabled without a bearer token and `allow_tokenless` is not set.
    #[error("no debug token configured: {0}")]
    MissingToken(String),
}

/// Commands that the debug surface can send to the phone runtime.
#[derive(Debug)]
pub enum RuntimeCommand {
    /// Inject a simulated core event into the runtime event loop.
    InjectEvent(Event),
    /// Ask the runtime to shut down.
    Shutdown,
    /// Ask the runtime for its current canonical state snapshot.
    Snapshot(oneshot::Sender<booth_core::State>),
}

/// State object returned by `GET /v1/state`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusSnapshot {
    /// Operator-compatible state name.
    pub state: String,
    /// RFC3339 timestamp when the state was observed by the debug surface.
    pub updated_at: String,
    /// Current question id, when known.
    pub current_question_id: Option<String>,
    /// Current message id, when known.
    pub current_message_id: Option<String>,
    /// Most recent error, when known.
    pub last_error: Option<String>,
}

/// Snapshot returned by `GET /v1/gpio`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpioSnapshot {
    /// Per-pin snapshots keyed by logical role.
    pub pins: Vec<GpioPinSnapshot>,
    /// RFC3339 timestamp for the newest GPIO edge included in the snapshot.
    pub updated_at: Option<String>,
}

/// Per-pin GPIO snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpioPinSnapshot {
    /// Logical role of the pin.
    pub role: PinRole,
    /// Most recent debounced level.
    pub level: bool,
    /// Alias for `level`, kept explicit for UI clarity.
    pub debounced_state: bool,
    /// Runtime monotonic timestamp for the last edge, in nanoseconds.
    pub last_edge_monotonic_ns: u64,
    /// Telemetry record id that carried the last edge.
    pub last_event_id: u64,
}

/// Snapshot returned by `GET /v1/audio`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioMeterSnapshot {
    /// Latest input RMS level in dBFS, clamped to -120 when silent.
    pub input_level_dbfs: f32,
    /// Latest output RMS level in dBFS, clamped to -120 when silent.
    pub output_level_dbfs: f32,
    /// Latest input peak level in dBFS, clamped to -120 when silent.
    pub input_peak_dbfs: f32,
    /// Latest output peak level in dBFS, clamped to -120 when silent.
    pub output_peak_dbfs: f32,
    /// Most recently reported device name, when known.
    pub current_device: Option<String>,
    /// Configured sample rate, when known.
    pub sample_rate_hz: Option<u32>,
    /// RFC3339 timestamp for the newest audio event included in the snapshot.
    pub updated_at: Option<String>,
}

impl Default for AudioMeterSnapshot {
    fn default() -> Self {
        Self {
            input_level_dbfs: floor_dbfs(),
            output_level_dbfs: floor_dbfs(),
            input_peak_dbfs: floor_dbfs(),
            output_peak_dbfs: floor_dbfs(),
            current_device: None,
            sample_rate_hz: None,
            updated_at: None,
        }
    }
}

/// One captured tracing log line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// RFC3339 timestamp assigned by the debug layer.
    pub ts: String,
    /// Lowercase tracing level.
    pub level: String,
    /// Tracing target/module path.
    pub target: String,
    /// Rendered log message and fields.
    pub message: String,
}

/// Addresses and task handle returned by [`serve_with_handles`].
#[derive(Debug)]
pub struct ServeHandles {
    /// Task that owns all enabled listeners.
    pub handle: JoinHandle<()>,
    /// Actual loopback listener address, useful when binding port 0 in tests.
    pub loopback_addr: Option<SocketAddr>,
    /// Actual LAN listener address, useful when binding port 0 in tests.
    pub lan_addr: Option<SocketAddr>,
    /// SHA-256 fingerprint of the generated LAN certificate.
    pub cert_fingerprint: String,
    /// Sender that signals graceful shutdown of all listener tasks.
    /// Dropping or sending on this channel causes each axum listener to
    /// stop accepting new connections and drain in-flight requests.
    pub shutdown_tx: oneshot::Sender<()>,
}

/// Renderer that produces Prometheus text exposition for the loopback
/// `/metrics` route. Supplied by the runtime so `booth-debug` stays
/// decoupled from the concrete metrics registry implementation.
pub type MetricsRender = Arc<dyn Fn() -> String + Send + Sync>;

#[derive(Clone)]
struct AppState {
    config: Arc<DebugConfig>,
    bus: TelemetryBus,
    runtime_tx: mpsc::Sender<RuntimeCommand>,
    cert_fingerprint: Arc<String>,
}

/// Start enabled debug listeners and return a task handle that owns them.
pub async fn serve(
    config: DebugConfig,
    bus: TelemetryBus,
    runtime_tx: mpsc::Sender<RuntimeCommand>,
) -> Result<JoinHandle<()>, DebugError> {
    Ok(serve_with_handles(config, bus, runtime_tx, None)
        .await?
        .handle)
}

/// Start enabled debug listeners and return bound addresses along with the task handle.
///
/// When `metrics_render` is `Some`, a `GET /metrics` route is mounted on the
/// loopback listener (Prometheus text exposition). The LAN listener never
/// exposes `/metrics` so the route is gated behind the Tailscale ACL on
/// the loopback front door. The route also bypasses the bearer-token
/// middleware so vmagent can scrape without credentials.
pub async fn serve_with_handles(
    config: DebugConfig,
    bus: TelemetryBus,
    runtime_tx: mpsc::Sender<RuntimeCommand>,
    metrics_render: Option<MetricsRender>,
) -> Result<ServeHandles, DebugError> {
    global_logs().set_capacity(config.ring_buffer_capacity);

    // Fail closed: refuse to start if no bearer token is configured and the
    // caller has not explicitly opted into tokenless operation.
    if config.token.is_none() && !config.allow_tokenless {
        return Err(DebugError::MissingToken(
            "debug listener requires a bearer token; set BOOTH_DEBUG_TOKEN or \
             [debug].token, or pass allow_tokenless = true for local development"
                .to_string(),
        ));
    }

    let tls = generate_tls_config()?;
    let fingerprint = tls.fingerprint.clone();
    let state = AppState {
        config: Arc::new(config.clone()),
        bus,
        runtime_tx,
        cert_fingerprint: Arc::new(fingerprint.clone()),
    };
    let authed_router = build_router(state.clone());
    let loopback_router = metrics_render.map_or_else(
        || authed_router.clone(),
        |render| authed_router.clone().merge(build_metrics_router(render)),
    );
    let lan_router = authed_router;

    let loopback_listener = if config.tailscale_enabled {
        let listener = TcpListener::bind(&config.loopback_bind)
            .await
            .map_err(|err| DebugError::Bind(format!("{}: {err}", config.loopback_bind)))?;
        Some(listener)
    } else {
        None
    };
    let loopback_addr = listener_addr(loopback_listener.as_ref())?;

    let lan_listener = if config.lan_enabled {
        validate_lan_security(&config)?;
        let listener = TcpListener::bind(&config.lan_bind)
            .await
            .map_err(|err| DebugError::Bind(format!("{}: {err}", config.lan_bind)))?;
        Some(TlsListener::new(listener, tls.acceptor))
    } else {
        None
    };
    let lan_addr = listener_addr(lan_listener.as_ref())?;

    if loopback_listener.is_none() && lan_listener.is_none() {
        return Err(DebugError::Bind("no debug listeners enabled".to_string()));
    }

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let handle = tokio::spawn(async move {
        // Convert the oneshot into a shared future so both listeners can
        // observe the same shutdown signal.
        let shutdown = async move {
            let _ = shutdown_rx.await;
        };
        let shutdown = shutdown.shared();

        let mut tasks = Vec::new();
        if let Some(listener) = loopback_listener {
            let service = loopback_router.into_make_service_with_connect_info::<DebugConnectInfo>();
            let signal = shutdown.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(err) = axum::serve(listener, service)
                    .with_graceful_shutdown(signal)
                    .await
                {
                    tracing::error!(error = %err, "loopback debug server stopped");
                }
            }));
        }
        if let Some(listener) = lan_listener {
            let service = lan_router.into_make_service_with_connect_info::<DebugConnectInfo>();
            let signal = shutdown.clone();
            tasks.push(tokio::spawn(async move {
                if let Err(err) = axum::serve(listener, service)
                    .with_graceful_shutdown(signal)
                    .await
                {
                    tracing::error!(error = %err, "lan debug server stopped");
                }
            }));
        }

        for task in tasks {
            if let Err(err) = task.await {
                tracing::error!(error = %err, "debug listener task panicked");
            }
        }
    });

    Ok(ServeHandles {
        handle,
        loopback_addr,
        lan_addr,
        cert_fingerprint: fingerprint,
        shutdown_tx,
    })
}

fn listener_addr<L>(listener: Option<&L>) -> Result<Option<SocketAddr>, DebugError>
where
    L: axum::serve::Listener<Addr = SocketAddr>,
{
    listener
        .map(axum::serve::Listener::local_addr)
        .transpose()
        .map_err(|err| DebugError::Bind(err.to_string()))
}

/// Minimum token length required for non-loopback LAN binds.
const MIN_LAN_TOKEN_LENGTH: usize = 16;

/// Reject non-loopback LAN bind addresses when the configured token is
/// missing or too short to be considered secure.
fn validate_lan_security(config: &DebugConfig) -> Result<(), DebugError> {
    let addr: SocketAddr = config
        .lan_bind
        .parse()
        .map_err(|err| DebugError::Bind(format!("invalid lan_bind address: {err}")))?;

    if addr.ip().is_loopback() {
        return Ok(());
    }

    // Any non-loopback address (including 0.0.0.0) requires a strong token.
    match &config.token {
        None => Err(DebugError::InsecureLanBind(
            "LAN listener binds to a non-loopback address but no debug token is configured; \
             set [debug].token to a value of at least 16 characters"
                .to_string(),
        )),
        Some(token) if token.0.len() < MIN_LAN_TOKEN_LENGTH => {
            Err(DebugError::InsecureLanBind(format!(
                "LAN listener binds to a non-loopback address but the debug token is too short \
                 ({} chars); use at least {MIN_LAN_TOKEN_LENGTH} characters",
                token.0.len(),
            )))
        }
        Some(_) => Ok(()),
    }
}

fn build_router(state: AppState) -> Router {
    let cors = cors_layer(state.config.operator_origin.as_deref());
    // Authenticated API routes — require bearer token.
    let authed = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/state", get(state_snapshot))
        .route("/v1/events", get(events_since))
        .route("/v1/gpio", get(gpio_snapshot))
        .route("/v1/audio", get(audio_snapshot))
        .route("/v1/system", get(system_snapshot))
        .route("/v1/logs", get(logs))
        .route("/v1/config", get(config_redacted))
        .route("/v1/cert/fingerprint", get(cert_fingerprint))
        .route("/v1/simulate/event", post(simulate_event))
        .route("/v1/simulate/pulse", post(simulate_pulse))
        .route("/v1/ws/telemetry", get(ws_telemetry))
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));
    // Public routes — served without auth so the login page is reachable.
    let public = Router::new()
        .route("/", get(root_redirect))
        .route("/v1/ui/simulator", get(simulator_ui))
        .with_state(state);
    authed.merge(public).layer(cors)
}

/// Redirect the bare `/` path to the simulator UI.
async fn root_redirect() -> Response {
    axum::response::Redirect::to("/v1/ui/simulator").into_response()
}

/// Build a tiny sub-router that exposes Prometheus text exposition.
///
/// This router is intentionally separate from [`build_router`] so it can
/// be merged into the loopback listener without picking up the bearer-token
/// middleware. The LAN listener never sees this router, so `/metrics` is
/// only reachable through `tailscale serve` (loopback front door).
fn build_metrics_router(render: MetricsRender) -> Router {
    Router::new().route(
        "/metrics",
        get(move || {
            let render = Arc::clone(&render);
            async move {
                let body = render();
                (
                    [(
                        CONTENT_TYPE,
                        HeaderValue::from_static("text/plain; version=0.0.4"),
                    )],
                    body,
                )
            }
        }),
    )
}

fn cors_layer(origin: Option<&str>) -> CorsLayer {
    let base = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            HeaderName::from_static("sec-websocket-protocol"),
        ]);

    match origin.map(HeaderValue::from_str) {
        Some(Ok(value)) => base.allow_origin(value),
        _ => base,
    }
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
    version: &'static str,
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn state_snapshot(State(state): State<AppState>) -> Json<StatusSnapshot> {
    Json(current_status(&state).await)
}

async fn current_status(state: &AppState) -> StatusSnapshot {
    let (tx, rx) = oneshot::channel();
    if state
        .runtime_tx
        .try_send(RuntimeCommand::Snapshot(tx))
        .is_ok()
        && let Ok(Ok(status)) = tokio::time::timeout(DEFAULT_STATE_QUERY_TIMEOUT, rx).await
    {
        return status_from_core(&status, None);
    }

    status_from_telemetry(&state.bus).unwrap_or_else(|| status_from_hal(HalBoothStatus::Idle, None))
}

#[derive(Debug, Deserialize)]
struct EventsQuery {
    since: Option<u64>,
}

async fn events_since(
    State(state): State<AppState>,
    Query(query): Query<EventsQuery>,
) -> Json<Vec<TelemetryRecord>> {
    Json(state.bus.snapshot_since(query.since))
}

async fn gpio_snapshot(State(state): State<AppState>) -> Json<GpioSnapshot> {
    Json(snapshot_gpio(&state.bus))
}

async fn audio_snapshot(State(state): State<AppState>) -> Json<AudioMeterSnapshot> {
    let mut snapshot = snapshot_audio(&state.bus);
    snapshot.sample_rate_hz = sample_rate_from_config(&state.config.effective_config.audio);
    Json(snapshot)
}

async fn system_snapshot(State(state): State<AppState>) -> Response {
    let latest = state
        .bus
        .snapshot_since(None)
        .into_iter()
        .rev()
        .find_map(|record| match record.event {
            TelemetryEvent::SystemSample { snapshot, .. } => Some(*snapshot),
            _ => None,
        });
    latest.map_or_else(
        || StatusCode::NO_CONTENT.into_response(),
        |snapshot| Json::<SystemSnapshot>(snapshot).into_response(),
    )
}

#[derive(Debug, Deserialize)]
struct LogsQuery {
    level: Option<String>,
    limit: Option<usize>,
}

async fn logs(Query(query): Query<LogsQuery>) -> Json<Vec<LogEntry>> {
    Json(global_logs().snapshot(query.level.as_deref(), query.limit.unwrap_or(200)))
}

async fn config_redacted(State(state): State<AppState>) -> Json<ConfigRedacted> {
    let mut config = state.config.effective_config.clone();
    config.debug = DebugConfigRedacted::from_debug_config(&state.config);
    if config.operator.token.is_empty()
        && let Some(token) = &state.config.token
    {
        config.operator.token = redact_token(&token.0);
    }
    Json(config)
}

async fn cert_fingerprint(
    State(state): State<AppState>,
    ConnectInfo(remote_addr): ConnectInfo<DebugConnectInfo>,
) -> Response {
    if remote_addr.0.ip().is_loopback() {
        Json(CertFingerprintResponse {
            sha256: (*state.cert_fingerprint).clone(),
        })
        .into_response()
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

#[derive(Debug, Serialize)]
struct CertFingerprintResponse {
    sha256: String,
}

async fn simulate_event(
    State(state): State<AppState>,
    Json(event): Json<Event>,
) -> Result<Json<SimulateResponse>, StatusCode> {
    ensure_controls(&state)?;
    state
        .runtime_tx
        .send(RuntimeCommand::InjectEvent(event))
        .await
        .map_err(|_err| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(SimulateResponse {
        accepted: true,
        injected: 1,
    }))
}

#[derive(Debug, Deserialize)]
struct PulseRequest {
    count: u8,
}

async fn simulate_pulse(
    State(state): State<AppState>,
    Json(request): Json<PulseRequest>,
) -> Result<Json<SimulateResponse>, StatusCode> {
    ensure_controls(&state)?;
    let mut injected = 0_u16;
    for _ in 0..request.count {
        state
            .runtime_tx
            .send(RuntimeCommand::InjectEvent(Event::RotaryPulse))
            .await
            .map_err(|_err| StatusCode::SERVICE_UNAVAILABLE)?;
        injected = injected.saturating_add(1);
    }
    state
        .runtime_tx
        .send(RuntimeCommand::InjectEvent(Event::Tick))
        .await
        .map_err(|_err| StatusCode::SERVICE_UNAVAILABLE)?;
    injected = injected.saturating_add(1);
    Ok(Json(SimulateResponse {
        accepted: true,
        injected,
    }))
}

fn ensure_controls(state: &AppState) -> Result<(), StatusCode> {
    if state.config.allow_controls {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

#[derive(Debug, Serialize)]
struct SimulateResponse {
    accepted: bool,
    injected: u16,
}

/// Serve the self-contained simulator control UI.
///
/// Served without auth so the token-entry login page is always reachable.
/// The page itself gates API calls behind the bearer token entered by the
/// user, and the control endpoints (`/v1/simulate/*`) enforce
/// `allow_controls` server-side.
async fn simulator_ui() -> Response {
    (
        [(
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )],
        include_str!("simulator_ui.html"),
    )
        .into_response()
}

async fn ws_telemetry(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| telemetry_socket(socket, state))
}

async fn telemetry_socket(mut socket: WebSocket, state: AppState) {
    let mut receiver = state.bus.subscribe();
    let replay_from = read_replay_request(&mut socket).await;
    for record in state.bus.snapshot_since(replay_from) {
        if send_record(&mut socket, &record).await.is_err() {
            return;
        }
    }

    loop {
        match receiver.recv().await {
            Ok(record) => {
                if send_record(&mut socket, &record).await.is_err() {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_skipped)) => {}
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReplayRequest {
    replay_from: u64,
}

async fn read_replay_request(socket: &mut WebSocket) -> Option<u64> {
    match tokio::time::timeout(WS_REPLAY_WAIT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<ReplayRequest>(&text)
            .ok()
            .map(|request| request.replay_from),
        Ok(Some(Ok(Message::Binary(bytes)))) => serde_json::from_slice::<ReplayRequest>(&bytes)
            .ok()
            .map(|request| request.replay_from),
        _ => None,
    }
}

async fn send_record(socket: &mut WebSocket, record: &TelemetryRecord) -> Result<(), axum::Error> {
    let text = serde_json::to_string(record).map_err(axum::Error::new)?;
    socket.send(Message::Text(text.into())).await
}

async fn auth_middleware(
    State(state): State<AppState>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if req.method() == Method::OPTIONS {
        return next.run(req).await;
    }

    let remote_addr = req
        .extensions()
        .get::<ConnectInfo<DebugConnectInfo>>()
        .map(|info| info.0.0);
    if is_authorized(&state.config, req.headers(), remote_addr) {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

fn is_authorized(
    config: &DebugConfig,
    headers: &HeaderMap,
    remote_addr: Option<SocketAddr>,
) -> bool {
    if config.loopback_skip_auth && remote_addr.is_some_and(|addr| addr.ip().is_loopback()) {
        return true;
    }
    let Some(token) = &config.token else {
        // No token configured — only permit if explicitly opted in.
        return config.allow_tokenless;
    };
    bearer_tokens(headers).any(|candidate| constant_time_eq(candidate, &token.0))
}

fn bearer_tokens(headers: &HeaderMap) -> impl Iterator<Item = &str> {
    let auth_tokens = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let protocol_tokens = headers
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok())
        .into_iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter_map(|value| value.strip_prefix("bearer."));
    auth_tokens.into_iter().chain(protocol_tokens)
}

fn constant_time_eq(candidate: &str, expected: &str) -> bool {
    bool::from(candidate.as_bytes().ct_eq(expected.as_bytes()))
}

fn status_from_telemetry(bus: &TelemetryBus) -> Option<StatusSnapshot> {
    bus.snapshot_since(None)
        .into_iter()
        .rev()
        .find_map(|record| match record.event {
            TelemetryEvent::StateTransition { to, .. } => Some(StatusSnapshot {
                state: operator_state_name(&to),
                updated_at: system_time_to_rfc3339(record.ts),
                current_question_id: None,
                current_message_id: None,
                last_error: None,
            }),
            TelemetryEvent::Error { message, .. } => Some(StatusSnapshot {
                state: "error".to_string(),
                updated_at: system_time_to_rfc3339(record.ts),
                current_question_id: None,
                current_message_id: None,
                last_error: Some(message),
            }),
            _ => None,
        })
}

fn status_from_core(state: &booth_core::State, updated_at: Option<SystemTime>) -> StatusSnapshot {
    status_from_hal(state.status(), updated_at)
}

fn status_from_hal(status: HalBoothStatus, updated_at: Option<SystemTime>) -> StatusSnapshot {
    StatusSnapshot {
        state: match status {
            HalBoothStatus::Idle => "idle",
            HalBoothStatus::DialTone => "dialTone",
            HalBoothStatus::PlayingQuestion => "playingQuestion",
            HalBoothStatus::Recording => "recording",
            HalBoothStatus::Uploading => "uploading",
            HalBoothStatus::PlayingMessage => "playingMessage",
            HalBoothStatus::PlayingInstructions => "playingInstructions",
        }
        .to_string(),
        updated_at: system_time_to_rfc3339(updated_at.unwrap_or_else(SystemTime::now)),
        current_question_id: None,
        current_message_id: None,
        last_error: None,
    }
}

fn operator_state_name(name: &str) -> String {
    match name {
        "idle" | "Idle" => "idle",
        "dial_tone" | "DialTone" => "dialTone",
        "dialing" | "Dialing" => "dialing",
        "playing_question" | "PlayingQuestion" => "playingQuestion",
        "beep" | "Beep" => "beep",
        "recording" | "Recording" => "recording",
        "uploading" | "Uploading" => "uploading",
        "playing_message" | "PlayingMessage" => "playingMessage",
        "playing_instructions" | "PlayingInstructions" => "playingInstructions",
        "error" | "Error" => "error",
        other => other,
    }
    .to_string()
}

fn snapshot_gpio(bus: &TelemetryBus) -> GpioSnapshot {
    let mut pins = HashMap::<PinRole, GpioPinSnapshot>::new();
    let mut updated_at = None;

    for record in bus.snapshot_since(None) {
        if let TelemetryEvent::GpioEdge(GpioEdge {
            role,
            level,
            at_monotonic_ns,
        }) = record.event
        {
            updated_at = Some(system_time_to_rfc3339(record.ts));
            pins.insert(
                role,
                GpioPinSnapshot {
                    role,
                    level,
                    debounced_state: level,
                    last_edge_monotonic_ns: at_monotonic_ns,
                    last_event_id: record.id,
                },
            );
        }
    }

    let mut pins = pins.into_values().collect::<Vec<_>>();
    pins.sort_by_key(|pin| match pin.role {
        PinRole::Hook => 0,
        PinRole::RotaryRead => 1,
        PinRole::RotaryPulse => 2,
    });
    GpioSnapshot { pins, updated_at }
}

fn snapshot_audio(bus: &TelemetryBus) -> AudioMeterSnapshot {
    let mut snapshot = AudioMeterSnapshot::default();
    for record in bus.snapshot_since(None) {
        match record.event {
            TelemetryEvent::AudioLevel(level) => {
                update_audio_level(&mut snapshot, level);
                snapshot.updated_at = Some(system_time_to_rfc3339(record.ts));
            }
            TelemetryEvent::AudioDeviceChange { name, .. } => {
                snapshot.current_device = Some(name);
                snapshot.updated_at = Some(system_time_to_rfc3339(record.ts));
            }
            _ => {}
        }
    }
    snapshot
}

fn update_audio_level(snapshot: &mut AudioMeterSnapshot, level: AudioLevel) {
    match level.channel {
        AudioChannel::Input => {
            snapshot.input_level_dbfs = amplitude_to_dbfs(level.rms);
            snapshot.input_peak_dbfs = amplitude_to_dbfs(level.peak);
        }
        AudioChannel::Output => {
            snapshot.output_level_dbfs = amplitude_to_dbfs(level.rms);
            snapshot.output_peak_dbfs = amplitude_to_dbfs(level.peak);
        }
    }
}

fn amplitude_to_dbfs(value: f32) -> f32 {
    if value <= 0.0 {
        floor_dbfs()
    } else {
        (20.0 * value.clamp(0.0, 1.0).log10()).max(floor_dbfs())
    }
}

fn floor_dbfs() -> f32 {
    -120.0
}

fn sample_rate_from_config(value: &serde_json::Value) -> Option<u32> {
    value
        .get("sampleRateHz")
        .or_else(|| value.get("sample_rate_hz"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn system_time_to_rfc3339(ts: SystemTime) -> String {
    OffsetDateTime::from(ts)
        .format(&Rfc3339)
        .unwrap_or_else(|_err| "1970-01-01T00:00:00Z".to_string())
}

struct TlsMaterial {
    acceptor: TlsAcceptor,
    fingerprint: String,
}

fn generate_tls_config() -> Result<TlsMaterial, DebugError> {
    let CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(["localhost".to_string(), "127.0.0.1".to_string()])
            .map_err(|err| DebugError::Tls(err.to_string()))?;
    let cert_der = cert.der().clone();
    let fingerprint = sha256_fingerprint(cert_der.as_ref());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));
    let mut server_config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|err| DebugError::Tls(err.to_string()))?
    .with_no_client_auth()
    .with_single_cert(vec![CertificateDer::from(cert_der.to_vec())], key_der)
    .map_err(|err| DebugError::Tls(err.to_string()))?;
    server_config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsMaterial {
        acceptor: TlsAcceptor::from(Arc::new(server_config)),
        fingerprint,
    })
}

fn sha256_fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

struct TlsListener {
    inner: TcpListener,
    acceptor: TlsAcceptor,
}

impl TlsListener {
    fn new(inner: TcpListener, acceptor: TlsAcceptor) -> Self {
        Self { inner, acceptor }
    }
}

#[derive(Debug, Clone, Copy)]
struct DebugConnectInfo(SocketAddr);

impl Connected<axum::serve::IncomingStream<'_, TcpListener>> for DebugConnectInfo {
    fn connect_info(stream: axum::serve::IncomingStream<'_, TcpListener>) -> Self {
        Self(*stream.remote_addr())
    }
}

impl Connected<axum::serve::IncomingStream<'_, TlsListener>> for DebugConnectInfo {
    fn connect_info(stream: axum::serve::IncomingStream<'_, TlsListener>) -> Self {
        Self(*stream.remote_addr())
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.inner.accept().await {
                Ok((stream, addr)) => match self.acceptor.accept(stream).await {
                    Ok(tls_stream) => return (tls_stream, addr),
                    Err(err) => tracing::warn!(error = %err, "tls handshake failed"),
                },
                Err(err) => handle_accept_error(err).await,
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

async fn handle_accept_error(err: io::Error) {
    tracing::warn!(error = %err, "debug listener accept failed");
    tokio::time::sleep(Duration::from_secs(1)).await;
}

#[derive(Debug, Clone)]
struct LogStore {
    capacity: Arc<AtomicUsize>,
    entries: Arc<Mutex<VecDeque<LogEntry>>>,
}

impl LogStore {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: Arc::new(AtomicUsize::new(capacity)),
            entries: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
        }
    }

    fn set_capacity(&self, capacity: usize) {
        self.capacity.store(capacity, Ordering::Relaxed);
        let mut entries = self.entries.lock();
        while entries.len() > capacity {
            entries.pop_front();
        }
    }

    fn push(&self, entry: LogEntry) {
        let capacity = self.capacity.load(Ordering::Relaxed);
        if capacity == 0 {
            return;
        }
        let mut entries = self.entries.lock();
        while entries.len() >= capacity {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    fn snapshot(&self, min_level: Option<&str>, limit: usize) -> Vec<LogEntry> {
        let limit = limit.max(1);
        let min_priority = min_level.and_then(level_priority);
        let entries = self.entries.lock();
        let mut output = entries
            .iter()
            .filter(|entry| {
                min_priority.is_none_or(|min| {
                    level_priority(&entry.level).is_some_and(|level| level <= min)
                })
            })
            .rev()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        output.reverse();
        output
    }
}

fn level_priority(level: &str) -> Option<u8> {
    match level.to_ascii_lowercase().as_str() {
        "error" => Some(1),
        "warn" => Some(2),
        "info" => Some(3),
        "debug" => Some(4),
        "trace" => Some(5),
        _ => None,
    }
}

fn global_logs() -> &'static LogStore {
    static LOGS: OnceLock<LogStore> = OnceLock::new();
    LOGS.get_or_init(|| LogStore::new(default_ring()))
}

/// A tracing-subscriber layer that captures formatted log lines into `/v1/logs`.
#[derive(Debug, Clone)]
pub struct DebugLogLayer {
    store: LogStore,
}

/// Create a log capture layer backed by the global debug log ring.
pub fn log_layer<S>() -> impl Layer<S> + Clone
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    DebugLogLayer {
        store: global_logs().clone(),
    }
}

/// Create a log capture layer and set the global log ring capacity.
pub fn log_layer_with_capacity<S>(capacity: usize) -> impl Layer<S> + Clone
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    global_logs().set_capacity(capacity);
    log_layer()
}

impl<S> Layer<S> for DebugLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &TracingEvent<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = LogVisitor::default();
        event.record(&mut visitor);
        self.store.push(LogEntry {
            ts: system_time_to_rfc3339(SystemTime::now()),
            level: level_to_str(*metadata.level()).to_string(),
            target: metadata.target().to_string(),
            message: visitor.finish(),
        });
    }
}

#[derive(Default)]
struct LogVisitor {
    message: Option<String>,
    fields: Vec<String>,
}

impl LogVisitor {
    fn finish(self) -> String {
        let mut parts = Vec::new();
        if let Some(message) = self.message {
            parts.push(message);
        }
        parts.extend(self.fields);
        parts.join(" ")
    }
}

impl Visit for LogVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        } else {
            self.fields.push(format!("{}={value:?}", field.name()));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.fields.push(format!("{}={value}", field.name()));
        }
    }
}

fn level_to_str(level: Level) -> &'static str {
    match level {
        Level::ERROR => "error",
        Level::WARN => "warn",
        Level::INFO => "info",
        Level::DEBUG => "debug",
        Level::TRACE => "trace",
    }
}

#[cfg(test)]
mod tests {
    use super::{amplitude_to_dbfs, operator_state_name, redact_token};

    #[test]
    fn redacts_token_last_four() {
        assert_eq!(redact_token("abcdef"), "<redacted:cdef>");
        assert_eq!(redact_token(""), "<empty>");
    }

    #[test]
    fn maps_operator_state_names() {
        assert_eq!(operator_state_name("dial_tone"), "dialTone");
        assert_eq!(operator_state_name("PlayingQuestion"), "playingQuestion");
    }

    #[test]
    fn converts_silence_to_floor_dbfs() {
        assert!((amplitude_to_dbfs(0.0) - -120.0).abs() < f32::EPSILON);
    }

    #[allow(clippy::unwrap_used)]
    mod auth {
        use super::super::{DebugConfig, DebugToken, is_authorized};
        use axum::http::HeaderMap;
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};

        #[test]
        fn denies_when_no_token_and_tokenless_not_allowed() {
            let config = DebugConfig {
                token: None,
                allow_tokenless: false,
                ..Default::default()
            };
            let remote = Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5)),
                1234,
            ));
            assert!(!is_authorized(&config, &HeaderMap::new(), remote));
        }

        #[test]
        fn allows_when_no_token_and_tokenless_allowed() {
            let config = DebugConfig {
                token: None,
                allow_tokenless: true,
                ..Default::default()
            };
            let remote = Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5)),
                1234,
            ));
            assert!(is_authorized(&config, &HeaderMap::new(), remote));
        }

        #[test]
        fn denies_wrong_bearer_token() {
            let config = DebugConfig {
                token: Some(DebugToken("correct-token-value".to_string())),
                allow_tokenless: false,
                ..Default::default()
            };
            let mut headers = HeaderMap::new();
            headers.insert("authorization", "Bearer wrong-token".parse().unwrap());
            let remote = Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5)),
                1234,
            ));
            assert!(!is_authorized(&config, &headers, remote));
        }

        #[test]
        fn allows_correct_bearer_token() {
            let config = DebugConfig {
                token: Some(DebugToken("correct-token-value".to_string())),
                allow_tokenless: false,
                ..Default::default()
            };
            let mut headers = HeaderMap::new();
            headers.insert(
                "authorization",
                "Bearer correct-token-value".parse().unwrap(),
            );
            let remote = Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 5)),
                1234,
            ));
            assert!(is_authorized(&config, &headers, remote));
        }

        #[test]
        fn loopback_skip_auth_still_works() {
            let config = DebugConfig {
                token: Some(DebugToken("secret".to_string())),
                loopback_skip_auth: true,
                allow_tokenless: false,
                ..Default::default()
            };
            let loopback = Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1234));
            assert!(is_authorized(&config, &HeaderMap::new(), loopback));
        }
    }

    #[allow(clippy::unwrap_used)]
    mod startup_validation {
        use super::super::{DebugConfig, DebugError, DebugToken, serve_with_handles};
        use booth_telemetry::TelemetryBus;
        use tokio::sync::mpsc;

        #[tokio::test]
        async fn rejects_startup_without_token() {
            let config = DebugConfig {
                token: None,
                allow_tokenless: false,
                ..Default::default()
            };
            let bus = TelemetryBus::new(16);
            let (tx, _rx) = mpsc::channel(1);
            let result = serve_with_handles(config, bus, tx, None).await;
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(
                matches!(err, DebugError::MissingToken(_)),
                "expected MissingToken, got: {err}"
            );
        }

        #[tokio::test]
        async fn allows_startup_with_allow_tokenless() {
            let config = DebugConfig {
                token: None,
                allow_tokenless: true,
                tailscale_enabled: true,
                ..Default::default()
            };
            let bus = TelemetryBus::new(16);
            let (tx, _rx) = mpsc::channel(1);
            let result = serve_with_handles(config, bus, tx, None).await;
            // Should succeed (or fail for a reason other than MissingToken,
            // e.g. port already in use)
            match result {
                Ok(handles) => {
                    let _ = handles.shutdown_tx.send(());
                    let _ = handles.handle.await;
                }
                Err(DebugError::MissingToken(_)) => {
                    panic!("should not get MissingToken when allow_tokenless is true");
                }
                Err(_) => {} // Other errors (bind, tls) are acceptable in test
            }
        }

        #[tokio::test]
        async fn allows_startup_with_token() {
            let config = DebugConfig {
                token: Some(DebugToken("my-secret-debug-token".to_string())),
                allow_tokenless: false,
                tailscale_enabled: true,
                ..Default::default()
            };
            let bus = TelemetryBus::new(16);
            let (tx, _rx) = mpsc::channel(1);
            let result = serve_with_handles(config, bus, tx, None).await;
            match result {
                Ok(handles) => {
                    let _ = handles.shutdown_tx.send(());
                    let _ = handles.handle.await;
                }
                Err(DebugError::MissingToken(_)) => {
                    panic!("should not get MissingToken when token is provided");
                }
                Err(_) => {} // Other errors are acceptable in test
            }
        }
    }
}
