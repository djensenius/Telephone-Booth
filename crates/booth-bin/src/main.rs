//! CLI entry point for the Telephone Booth runtime.

#![warn(missing_docs)]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use booth_bin::{
    DEFAULT_CONFIG_PATH, RuntimeOptions, build_pi_adapters, check_runtime, load_config,
    render_config_toml, simulate_pulses, spawn_runtime,
};
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
}

#[tokio::main]
async fn main() -> ExitCode {
    install_tracing("info");
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
        Command::Run { config, mock } => {
            let config = load_config(config.as_deref())?;
            run(config, mock).await
        }
        Command::PrintConfig { config } => {
            let config = load_config(config.as_deref())?;
            print!("{}", render_config_toml(&config)?);
            Ok(())
        }
        Command::Check { config } => {
            let config = load_config(config.as_deref())?;
            check_runtime(&config).await
        }
        Command::Simulate { pulses } => {
            for (event, state, effects) in simulate_pulses(pulses) {
                println!("event={event:?} state={state:?} effects={effects:?}");
            }
            Ok(())
        }
    }
}

async fn run(config: booth_bin::RuntimeConfig, mock: bool) -> Result<()> {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        default_config = DEFAULT_CONFIG_PATH,
        mock,
        "starting telephone-booth runtime"
    );
    let bus = TelemetryBus::new(config.ring_buffer_capacity());
    let adapters = if mock {
        mock_adapters(&bus)?
    } else {
        build_pi_adapters(&config, &bus)?
    };
    let handle = spawn_runtime(config, adapters, bus, RuntimeOptions::default());
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
