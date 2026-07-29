//! Host power control backed by `systemctl` on the Raspberry Pi.
//!
//! The pure core never shells out: it emits `Effect::Reboot` / `Effect::PowerOff`
//! as data and the runtime routes those to the [`PowerController`] port. This
//! adapter fulfils the port by invoking `systemctl reboot` / `systemctl poweroff`.

use async_trait::async_trait;
use booth_hal::{PowerController, PowerError};
use tokio::process::Command;
use tracing::{info, warn};

/// [`PowerController`] that reboots / powers off the host via `systemctl`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PiPowerController;

impl PiPowerController {
    /// Create a new controller.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

async fn run_systemctl(action: &'static str) -> Result<(), PowerError> {
    info!(action, "invoking systemctl for power button");
    let status = Command::new("systemctl")
        .arg(action)
        .status()
        .await
        .map_err(|err| PowerError::Command(format!("spawn systemctl {action}: {err}").into()))?;
    if status.success() {
        Ok(())
    } else {
        warn!(action, ?status, "systemctl exited non-zero");
        Err(PowerError::Command(
            format!("systemctl {action} exited with {status}").into(),
        ))
    }
}

#[async_trait]
impl PowerController for PiPowerController {
    async fn reboot(&self) -> Result<(), PowerError> {
        run_systemctl("reboot").await
    }

    async fn poweroff(&self) -> Result<(), PowerError> {
        run_systemctl("poweroff").await
    }
}
