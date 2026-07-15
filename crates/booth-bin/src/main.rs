//! CLI entry point for the Telephone Booth runtime.

#![warn(missing_docs)]

use std::path::PathBuf;
use std::process::{Command as ProcessCommand, ExitCode};

use anyhow::{Context, Result, bail};
use booth_bin::{
    DEFAULT_CONFIG_PATH, RuntimeOptions, build_pi_adapters, check_runtime, load_config,
    render_config_toml, simulate_pulses, spawn_runtime,
};
use booth_hal::RuntimeMode;
use booth_telemetry::TelemetryBus;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

/// Telephone Booth phone-side runtime.
#[derive(Debug, Parser)]
#[command(name = "telephone-booth", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Supported runtime and diagnostic commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Start the HAL-backed runtime loop.
    Run {
        /// Config path to read. Defaults to /etc/phone-booth/config.toml, then ./config.toml.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Use in-memory mock adapters instead of Raspberry Pi hardware adapters.
        #[arg(long)]
        mock: bool,
        /// Launch the interactive TUI simulator that injects GPIO events from the
        /// keyboard. Pair with `--mock` to also mock audio and the operator
        /// client; without `--mock` the simulator drives the real cross-platform
        /// audio + HTTP adapters.
        #[cfg(feature = "simulator")]
        #[arg(long)]
        simulator: bool,
        /// Launch a read-only TUI monitor over the real hardware. Streams live
        /// telemetry (state, decoded digits, audio levels, operator calls) in a
        /// scrolling log while you dial the physical phone. Reserves the same
        /// GPIO/audio as the systemd service, so stop `telephone-booth.service`
        /// first. Pair with `--mock` to monitor mock adapters instead.
        #[cfg(feature = "simulator")]
        #[arg(long)]
        tui: bool,
    },
    /// Print the effective merged config as TOML with tokens redacted.
    PrintConfig {
        /// Config path to read. Defaults to /etc/phone-booth/config.toml, then ./config.toml.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Validate config and probe hardware adapters; intended for systemd ExecStartPre.
    Check {
        /// Config path to read. Defaults to /etc/phone-booth/config.toml, then ./config.toml.
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Run a local pure-state-machine rotary pulse diagnostic.
    Simulate {
        /// Number of rotary pulses to inject before the timeout tick.
        pulses: u8,
    },
    /// Print Tailscale MagicDNS, serve config, and health status.
    TailscaleStatus,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run_cli().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("telephone-booth: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run {
            config,
            mock,
            #[cfg(feature = "simulator")]
            simulator,
            #[cfg(feature = "simulator")]
            tui,
        } => {
            let config = load_config(config.as_deref())?;
            // CLI flag can only force a mode on; the config setting provides
            // the autostart baseline for systemd units.
            let mock = mock || config.runtime.mock;
            // An explicit `--tui` takes precedence over the simulator autostart
            // baseline, so a user asking for the read-only hardware monitor is
            // never silently redirected into the interactive simulator.
            #[cfg(feature = "simulator")]
            if tui {
                let (log_path, _guard) = install_simulator_tracing(&config.telemetry.journal_level);
                return booth_bin::simulator::run_monitor(config, mock, log_path).await;
            }
            #[cfg(feature = "simulator")]
            let simulator = simulator || config.runtime.simulator;
            #[cfg(feature = "simulator")]
            if simulator {
                let (log_path, _guard) = install_simulator_tracing(&config.telemetry.journal_level);
                return booth_bin::simulator::run_simulator(config, mock, log_path).await;
            }
            install_tracing(&config.telemetry.journal_level);
            run(config, mock).await
        }
        Command::PrintConfig { config } => {
            install_tracing("warn");
            let config = load_config(config.as_deref())?;
            print!("{}", render_config_toml(&config)?);
            Ok(())
        }
        Command::Check { config } => {
            install_tracing("info");
            let config = load_config(config.as_deref())?;
            check_runtime(&config).await
        }
        Command::Simulate { pulses } => {
            install_tracing("info");
            for (event, state, effects) in simulate_pulses(pulses) {
                println!("event={event:?} state={state:?} effects={effects:?}");
            }
            Ok(())
        }
        Command::TailscaleStatus => {
            install_tracing("warn");
            print_tailscale_status()
        }
    }
}

fn print_tailscale_status() -> Result<()> {
    let output = ProcessCommand::new("tailscale")
        .args(["status", "--json"])
        .output()
        .context("run tailscale status --json")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("tailscale status --json failed: {}", stderr.trim());
    }

    let status: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("parse tailscale status JSON")?;
    let magicdnsname = magic_dns_name(&status).unwrap_or_else(|| "<unknown>".to_string());

    println!("magicdnsname: {magicdnsname}");
    if magicdnsname == "<unknown>" {
        println!("url: <unknown>");
    } else {
        println!("url: https://{magicdnsname}");
    }

    println!("health:");
    print_health(status.get("Health"))?;

    println!("serve_config:");
    let serve_config = if let Some(value) = status
        .get("ServeConfig")
        .or_else(|| status.get("serve_config"))
        .or_else(|| status.get("Serve"))
    {
        Some(value.clone())
    } else {
        load_tailscale_serve_config()?
    };
    print_json_block(serve_config.as_ref())?;

    Ok(())
}

fn load_tailscale_serve_config() -> Result<Option<serde_json::Value>> {
    let output = ProcessCommand::new("tailscale")
        .args(["serve", "status", "--json"])
        .output()
        .context("run tailscale serve status --json")?;
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(None);
    }
    let status =
        serde_json::from_slice(&output.stdout).context("parse tailscale serve status JSON")?;
    Ok(Some(status))
}

fn magic_dns_name(status: &serde_json::Value) -> Option<String> {
    let value = status
        .pointer("/Self/DNSName")
        .or_else(|| status.get("MagicDNSName"))
        .or_else(|| status.get("magicdnsname"))
        .or_else(|| status.get("DNSName"))?;
    let name = value.as_str()?.trim().trim_end_matches('.');
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn print_health(value: Option<&serde_json::Value>) -> Result<()> {
    match value {
        Some(serde_json::Value::Array(items)) if items.is_empty() => println!("  ok"),
        Some(serde_json::Value::Array(items)) => {
            for item in items {
                if let Some(text) = item.as_str() {
                    println!("  - {text}");
                } else {
                    print_json_block(Some(item))?;
                }
            }
        }
        Some(serde_json::Value::Null) | None => println!("  <none reported>"),
        Some(value) => print_json_block(Some(value))?,
    }
    Ok(())
}

fn print_json_block(value: Option<&serde_json::Value>) -> Result<()> {
    let Some(value) = value else {
        println!("  <none>");
        return Ok(());
    };
    if value.is_null() {
        println!("  <none>");
        return Ok(());
    }
    let rendered = serde_json::to_string_pretty(value).context("render tailscale status JSON")?;
    for line in rendered.lines() {
        println!("  {line}");
    }
    Ok(())
}

async fn run(config: booth_bin::RuntimeConfig, mock: bool) -> Result<()> {
    let runtime_mode = if mock {
        RuntimeMode::Mock
    } else {
        RuntimeMode::Real
    };
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        default_config = DEFAULT_CONFIG_PATH,
        mock,
        runtime_mode = %runtime_mode,
        "starting telephone-booth runtime"
    );
    let bus = TelemetryBus::new(config.ring_buffer_capacity());
    let adapters = if mock {
        mock_adapters(&bus)?
    } else {
        build_pi_adapters(&config, &bus, runtime_mode)?
    };
    let handle = spawn_runtime(
        config,
        adapters,
        bus,
        RuntimeOptions {
            runtime_mode,
            ..RuntimeOptions::default()
        },
    );
    let final_state = handle.join.await.context("runtime task panicked")??;
    tracing::info!(state = final_state.tag(), "runtime stopped");
    Ok(())
}

#[cfg(feature = "mock")]
#[allow(
    clippy::unnecessary_wraps,
    reason = "the no-mock cfg returns an error; keep one call shape for run()"
)]
fn mock_adapters(bus: &TelemetryBus) -> Result<booth_bin::RuntimeAdapters> {
    let (adapters, _handles) = booth_bin::build_mock_adapters(bus);
    Ok(adapters)
}

#[cfg(not(feature = "mock"))]
fn mock_adapters(_bus: &TelemetryBus) -> Result<booth_bin::RuntimeAdapters> {
    anyhow::bail!("--mock requires booth-bin to be built with the `mock` feature")
}

fn install_tracing(default_filter: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(true))
        .with(booth_debug::log_layer());
    let _ = tracing::subscriber::set_global_default(subscriber);
}

/// Install a file-only tracing subscriber for the simulator so log output does
/// not corrupt the TUI. Returns the resolved log path and the worker guard
/// (which must be held for the lifetime of the program to ensure flush).
#[cfg(feature = "simulator")]
fn install_simulator_tracing(
    default_filter: &str,
) -> (
    Option<String>,
    Option<tracing_appender::non_blocking::WorkerGuard>,
) {
    let path = std::env::var("BOOTH_SIM_LOG_PATH")
        .unwrap_or_else(|_| "/tmp/telephone-booth-sim.log".to_string());
    let path_buf = PathBuf::from(&path);
    let parent = path_buf
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let file_name = path_buf.file_name().map_or_else(
        || "telephone-booth-sim.log".to_string(),
        |s| s.to_string_lossy().into_owned(),
    );

    let appender = tracing_appender::rolling::never(parent, &file_name);
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(true)
                .with_ansi(false)
                .with_writer(writer),
        )
        .with(booth_debug::log_layer());
    let _ = tracing::subscriber::set_global_default(subscriber);

    // Re-point the process's stderr at the same log file so C-level libraries
    // (e.g. alsa-lib's snd_lib_error_default) can't punch through the TUI by
    // writing directly to FD 2. Failure here is non-fatal — we'd rather still
    // launch the TUI than abort startup over a logging side-channel.
    #[cfg(unix)]
    redirect_stderr_to(&path_buf);

    (Some(path), Some(guard))
}

/// Replace this process's stderr (FD 2) with a handle to `path`. Used by the
/// simulator TUI so that out-of-band C-library writers (alsa-lib, libsystemd,
/// etc.) end up in the simulator log file instead of corrupting the
/// framebuffer. Best-effort: silently no-ops if the file can't be opened or
/// the `dup2` call fails.
#[cfg(all(unix, feature = "simulator"))]
fn redirect_stderr_to(path: &std::path::Path) {
    use std::fs::OpenOptions;

    let Ok(file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = rustix::stdio::dup2_stderr(&file);
}
