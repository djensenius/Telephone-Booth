//! Interactive TUI that drives the phone runtime through a mocked GPIO port.
//!
//! The simulator gives developers a way to exercise the full booth pipeline —
//! the state machine, audio playback/capture, and the operator HTTP client —
//! from a development machine that has no rotary phone hardware attached.
//!
//! Hardware events (hook lift, dial pulses) are synthesized by keypresses and
//! injected into a [`booth_mock::MockGpioPort`]. Audio and the operator
//! client are either real (`PiAudioSink`/`PiAudioSource`/`PiOperatorClient`)
//! or mock, depending on whether `--mock` was passed alongside `--simulator`.

#![cfg(feature = "simulator")]

use std::collections::VecDeque;
use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::{Context, Result};
use booth_debug::RuntimeCommand;
use booth_hal::{AudioChannel, GpioEdge, PinRole, TelemetryEvent};
use booth_telemetry::{TelemetryBus, TelemetryRecord};
use crossterm::event::{Event as CtEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap};
use time::OffsetDateTime;
use time::format_description::well_known::Iso8601;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{Instant, MissedTickBehavior, interval};

use crate::{RuntimeConfig, RuntimeOptions, build_simulator_adapters, spawn_runtime};
use booth_hal::RuntimeMode;

const EVENT_HISTORY: usize = 64;
const RENDER_TICK: Duration = Duration::from_millis(100);
const DEFAULT_OPERATOR_URL: &str = "https://operator.example.com";

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

    let mut terminal = TerminalGuard::enter().context("enter terminal alternate screen")?;
    let mut state = SimulatorState::new(mock_io, log_path);
    let mut telemetry_rx = bus.subscribe();
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
                        if matches!(state.handle_key(key, &injector).await, Action::Quit) {
                            let _ = handle.commands.send(RuntimeCommand::Shutdown).await;
                            break Ok(());
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => break Err(err).context("read terminal event"),
                    None => break Ok(()),
                }
            }
            record = telemetry_rx.recv() => {
                match record {
                    Ok(record) => state.ingest(&record),
                    Err(RecvError::Lagged(skipped)) => {
                        state.note_lag(skipped);
                    }
                    Err(RecvError::Closed) => break Ok(()),
                }
            }
            _ = ticker.tick() => {}
        }
        terminal.draw(&state)?;
    };

    // Always restore the terminal before printing or returning.
    drop(terminal);

    // Wait briefly for the runtime to finish cleanly. The runtime task
    // exits when it sees Shutdown above.
    match tokio::time::timeout(Duration::from_secs(2), handle.join).await {
        Ok(Ok(Ok(final_state))) => {
            tracing::info!(state = final_state.tag(), "simulator runtime stopped");
        }
        Ok(Ok(Err(err))) => tracing::warn!(error = %err, "runtime exited with error"),
        Ok(Err(join_err)) => tracing::warn!(error = %join_err, "runtime task panicked"),
        Err(_) => tracing::warn!("runtime did not stop within 2s of shutdown"),
    }

    outcome
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
    log_path: Option<String>,
    hook_on_hook: bool,
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
    fn new(mock_io: bool, log_path: Option<String>) -> Self {
        Self {
            mock_io,
            log_path,
            hook_on_hook: true,
            current_state: "idle".to_string(),
            booth_status: "idle".to_string(),
            audio_in: LevelView::default(),
            audio_out: LevelView::default(),
            history: VecDeque::with_capacity(EVENT_HISTORY),
            status_line: "Press [h] or space to lift the receiver.".to_string(),
            lagged_records: 0,
            started_at: Instant::now(),
        }
    }

    async fn handle_key(&mut self, key: KeyEvent, injector: &booth_mock::GpioInjector) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            KeyCode::Char('c') if ctrl => return Action::Quit,
            KeyCode::Char('h' | ' ') => self.toggle_hook(injector).await,
            KeyCode::Char(c @ '0'..='9') => {
                if self.hook_on_hook {
                    self.set_status(
                        "Lift the receiver before dialing (press [h] or space).",
                        Style::default().fg(Color::Yellow),
                    );
                } else if let Some(digit) = c.to_digit(10).and_then(|d| u8::try_from(d).ok()) {
                    self.dial_digit(digit, injector).await;
                }
            }
            _ => {}
        }
        Action::Continue
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
        let hook = if self.hook_on_hook {
            "on-hook"
        } else {
            "off-hook"
        };
        let header = Line::from(vec![
            Span::styled(
                "Telephone Booth Simulator",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(format!("[{mode}]"), Style::default().fg(Color::DarkGray)),
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
                Style::default().fg(if self.hook_on_hook {
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
        let controls = "Controls: [h]/space toggle hook   [0-9] dial digit   [q]/Esc/Ctrl+C quit";
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
