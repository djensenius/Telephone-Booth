//! `telephone-booth` — the runtime that wires HAL adapters into the
//! [`booth_core`] state machine and drives the booth.
//!
//! The runtime loop is intentionally tiny:
//!
//! 1. Read a HAL event from any source (GPIO edge, playback end, upload
//!    completion, tick, …) into a [`booth_core::Event`].
//! 2. Call [`booth_core::handle`] to compute the next state and a `Vec` of
//!    [`booth_core::Effect`]s.
//! 3. Dispatch each effect against the appropriate HAL trait.
//! 4. Publish a `StateTransition` onto the telemetry bus consumed by
//!    [`booth_debug`].
//!
//! The concrete loop, signal handling, and adapter wiring is added by the
//! `rust-bin-wiring` agent task. This `main.rs` is a minimal placeholder so
//! the workspace compiles and `--print-config` works.

#![warn(missing_docs)]

use std::process::ExitCode;

use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .init();

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--print-config") {
        let cfg = booth_pi::PiConfig::default();
        match toml::to_string_pretty(&cfg) {
            Ok(s) => {
                print!("{s}");
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!("failed to render config: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    tracing::info!(
        "telephone-booth {} starting (placeholder runtime; see docs/runbook.md)",
        env!("CARGO_PKG_VERSION")
    );
    tracing::warn!("runtime wiring not yet present (filled in by `rust-bin-wiring` task)");

    ExitCode::SUCCESS
}
