//! GPIO adapter backed by `rppal` on Raspberry Pi hardware.

use async_trait::async_trait;
use booth_hal::{GpioEdge, GpioError, GpioPort, PinRole};

use crate::GpioConfig;

#[cfg(all(feature = "pi", target_os = "linux"))]
mod imp {
    use std::time::{Duration, Instant};

    use super::{GpioConfig, GpioEdge, GpioError, GpioPort, PinRole, async_trait};
    use crate::GpioPull;
    use rppal::gpio::{Event, Gpio, InputPin, Trigger};
    use tokio::runtime::Handle;
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;
    use tracing::{debug, error, info, warn};

    /// Raspberry Pi GPIO implementation for the booth input pins.
    pub struct PiGpioPort {
        _gpio: Gpio,
        config: GpioConfig,
        pins: PiPins,
        rx: mpsc::UnboundedReceiver<GpioEdge>,
        debounce_tasks: Vec<JoinHandle<()>>,
        started_at: Instant,
    }

    struct PiPins {
        hook: InputPin,
        rotary_pulse: InputPin,
        rotary_read: InputPin,
    }

    impl PiGpioPort {
        /// Open the configured BCM pins, configure interrupts, and start debouncing.
        pub fn new(config: GpioConfig) -> Result<Self, GpioError> {
            let handle = Handle::try_current().map_err(|err| {
                GpioError::Setup(
                    format!("tokio runtime required for gpio debounce tasks: {err}").into(),
                )
            })?;
            let gpio = Gpio::new()
                .map_err(|err| GpioError::Setup(format!("failed to open gpio: {err}").into()))?;
            let mut pins = PiPins {
                hook: open_input(&gpio, &config, PinRole::Hook)?,
                rotary_pulse: open_input(&gpio, &config, PinRole::RotaryPulse)?,
                rotary_read: open_input(&gpio, &config, PinRole::RotaryRead)?,
            };

            let (tx, rx) = mpsc::unbounded_channel();
            let started_at = Instant::now();
            let debounce_tasks = vec![
                configure_interrupt(
                    &mut pins.hook,
                    PinRole::Hook,
                    &config,
                    &handle,
                    tx.clone(),
                    started_at,
                )?,
                configure_interrupt(
                    &mut pins.rotary_pulse,
                    PinRole::RotaryPulse,
                    &config,
                    &handle,
                    tx.clone(),
                    started_at,
                )?,
                configure_interrupt(
                    &mut pins.rotary_read,
                    PinRole::RotaryRead,
                    &config,
                    &handle,
                    tx,
                    started_at,
                )?,
            ];

            info!(
                hook_bcm = config.bcm_for(PinRole::Hook),
                rotary_pulse_bcm = config.bcm_for(PinRole::RotaryPulse),
                rotary_read_bcm = config.bcm_for(PinRole::RotaryRead),
                debounce_ms = config.debounce_ms,
                pull = ?config.pull,
                "configured raspberry pi gpio inputs"
            );

            Ok(Self {
                _gpio: gpio,
                config,
                pins,
                rx,
                debounce_tasks,
                started_at,
            })
        }
    }

    #[async_trait]
    impl GpioPort for PiGpioPort {
        async fn next_edge(&mut self) -> Result<GpioEdge, GpioError> {
            self.rx
                .recv()
                .await
                .ok_or_else(|| GpioError::Stream("gpio event channel closed".into()))
        }

        async fn snapshot(&self, role: PinRole) -> Result<bool, GpioError> {
            let pin = match role {
                PinRole::Hook => &self.pins.hook,
                PinRole::RotaryPulse => &self.pins.rotary_pulse,
                PinRole::RotaryRead => &self.pins.rotary_read,
            };

            Ok(apply_invert(pin.is_high(), self.config.inverted(role)))
        }
    }

    impl Drop for PiGpioPort {
        fn drop(&mut self) {
            clear_interrupt(PinRole::Hook, &mut self.pins.hook);
            clear_interrupt(PinRole::RotaryPulse, &mut self.pins.rotary_pulse);
            clear_interrupt(PinRole::RotaryRead, &mut self.pins.rotary_read);

            for task in self.debounce_tasks.drain(..) {
                task.abort();
            }

            debug!(
                elapsed_ns = monotonic_ns(self.started_at.elapsed()),
                "released raspberry pi gpio inputs"
            );
        }
    }

    fn open_input(gpio: &Gpio, config: &GpioConfig, role: PinRole) -> Result<InputPin, GpioError> {
        let bcm = config.bcm_for(role);
        let pin = gpio.get(bcm).map_err(|err| {
            GpioError::Setup(format!("failed to open {role:?} BCM {bcm}: {err}").into())
        })?;

        Ok(match config.pull {
            GpioPull::Up => pin.into_input_pullup(),
            GpioPull::Down => pin.into_input_pulldown(),
        })
    }

    fn configure_interrupt(
        pin: &mut InputPin,
        role: PinRole,
        config: &GpioConfig,
        handle: &Handle,
        tx: mpsc::UnboundedSender<GpioEdge>,
        started_at: Instant,
    ) -> Result<JoinHandle<()>, GpioError> {
        let (raw_tx, raw_rx) = mpsc::unbounded_channel();
        let debounce = Duration::from_millis(config.debounce_ms);
        let task = handle.spawn(debounce_edges(role, raw_rx, tx, debounce, started_at));
        let invert = config.inverted(role);
        let bcm = config.bcm_for(role);

        pin.set_async_interrupt(Trigger::Both, None, move |event| {
            if let Some(level) = event_level(event, invert) {
                debug!(
                    ?role,
                    bcm,
                    seqno = event.seqno,
                    trigger = ?event.trigger,
                    level,
                    "gpio interrupt"
                );

                if raw_tx.send(level).is_err() {
                    warn!(?role, bcm, "gpio debounce task already stopped");
                }
            } else {
                warn!(
                    ?role,
                    bcm,
                    trigger = ?event.trigger,
                    "ignored gpio interrupt with unsupported trigger"
                );
            }
        })
        .map_err(|err| {
            GpioError::Setup(
                format!("failed to register {role:?} BCM {bcm} interrupt: {err}").into(),
            )
        })?;

        Ok(task)
    }

    async fn debounce_edges(
        role: PinRole,
        mut raw_rx: mpsc::UnboundedReceiver<bool>,
        tx: mpsc::UnboundedSender<GpioEdge>,
        debounce: Duration,
        started_at: Instant,
    ) {
        let mut last_forwarded = None;

        while let Some(mut pending_level) = raw_rx.recv().await {
            loop {
                tokio::select! {
                    next_level = raw_rx.recv() => {
                        let Some(next_level) = next_level else {
                            debug!(?role, "gpio raw edge channel closed");
                            return;
                        };
                        pending_level = next_level;
                    }
                    () = tokio::time::sleep(debounce) => {
                        if last_forwarded == Some(pending_level) {
                            debug!(?role, level = pending_level, "suppressed duplicate gpio edge");
                        } else {
                            last_forwarded = Some(pending_level);
                            let edge = GpioEdge {
                                role,
                                level: pending_level,
                                at_monotonic_ns: monotonic_ns(started_at.elapsed()),
                            };

                            if tx.send(edge).is_err() {
                                error!(?role, "gpio edge receiver dropped");
                                return;
                            }

                            debug!(?role, level = pending_level, "forwarded debounced gpio edge");
                        }
                        break;
                    }
                }
            }
        }
    }

    fn event_level(event: Event, invert: bool) -> Option<bool> {
        let physical_high = match event.trigger {
            Trigger::RisingEdge => true,
            Trigger::FallingEdge => false,
            Trigger::Disabled | Trigger::Both => return None,
        };

        Some(apply_invert(physical_high, invert))
    }

    fn apply_invert(physical_high: bool, invert: bool) -> bool {
        if invert {
            !physical_high
        } else {
            physical_high
        }
    }

    fn clear_interrupt(role: PinRole, pin: &mut InputPin) {
        if let Err(err) = pin.clear_async_interrupt() {
            warn!(?role, "failed to clear gpio interrupt: {err}");
        }
    }

    fn monotonic_ns(elapsed: Duration) -> u64 {
        u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
    }
}

#[cfg(not(all(feature = "pi", target_os = "linux")))]
mod imp {
    use super::{GpioConfig, GpioEdge, GpioError, GpioPort, PinRole, async_trait};

    /// Stub GPIO implementation used when the `pi` feature is disabled or the
    /// target is not Linux (rppal is Linux-only).
    pub struct PiGpioPort;

    impl PiGpioPort {
        /// Return an unsupported error because real GPIO requires the `pi`
        /// feature on a Linux target.
        pub fn new(_config: GpioConfig) -> Result<Self, GpioError> {
            Err(GpioError::Unsupported(
                "booth-pi gpio requires the `pi` feature on a Linux target".into(),
            ))
        }
    }

    #[async_trait]
    impl GpioPort for PiGpioPort {
        async fn next_edge(&mut self) -> Result<GpioEdge, GpioError> {
            Err(GpioError::Unsupported(
                "booth-pi gpio requires the `pi` feature on a Linux target".into(),
            ))
        }

        async fn snapshot(&self, _role: PinRole) -> Result<bool, GpioError> {
            Err(GpioError::Unsupported(
                "booth-pi gpio requires the `pi` feature on a Linux target".into(),
            ))
        }
    }
}

pub use imp::PiGpioPort;
