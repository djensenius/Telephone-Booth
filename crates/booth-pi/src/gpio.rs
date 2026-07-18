//! GPIO adapter backed by `rppal` on Raspberry Pi hardware.

use async_trait::async_trait;
use booth_hal::{GpioEdge, GpioError, GpioPort, PinRole};

use crate::GpioConfig;

#[cfg(all(feature = "pi", target_os = "linux"))]
mod imp {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use super::{GpioConfig, GpioEdge, GpioError, GpioPort, PinRole, async_trait};
    use crate::GpioPull;
    use rppal::gpio::{Gpio, InputPin};
    use tokio::runtime::Handle;
    use tokio::sync::mpsc;
    use tokio::task::JoinHandle;
    use tracing::{debug, info, warn};

    /// Interval between GPIO level samples.
    ///
    /// rppal's asynchronous interrupts read edges through the Linux GPIO
    /// character device (cdev `uAPI v1`). On some Raspberry Pi OS kernels
    /// (observed on a Pi 4 running kernel 6.18) that event fd immediately
    /// reports an error, so rppal's interrupt worker thread exits and silently
    /// drops the edge callback — the booth then sees the edge stream close
    /// within a millisecond of every (re)start. Polling the memory-mapped level
    /// registers via [`InputPin::is_high`] sidesteps the cdev event path
    /// entirely, is cheap on a Pi, and keeps the internal pull resistor applied
    /// (a cdev line-event request resets the pin bias to none). Two
    /// milliseconds is far finer than the ~33 ms make phase of a 10
    /// pulse-per-second rotary dial.
    const POLL_INTERVAL: Duration = Duration::from_millis(2);

    /// Logical input roles, in a fixed order that matches [`role_index`].
    const ROLES: [PinRole; 3] = [PinRole::Hook, PinRole::RotaryPulse, PinRole::RotaryRead];

    const fn role_index(role: PinRole) -> usize {
        match role {
            PinRole::Hook => 0,
            PinRole::RotaryPulse => 1,
            PinRole::RotaryRead => 2,
        }
    }

    /// Raspberry Pi GPIO implementation for the booth input pins.
    pub struct PiGpioPort {
        rx: mpsc::Receiver<GpioEdge>,
        levels: Arc<[AtomicBool; 3]>,
        poll_task: JoinHandle<()>,
        started_at: Instant,
    }

    struct PiPins {
        _gpio: Gpio,
        hook: InputPin,
        rotary_pulse: InputPin,
        rotary_read: InputPin,
    }

    impl PiPins {
        fn physical_high(&self, role: PinRole) -> bool {
            match role {
                PinRole::Hook => self.hook.is_high(),
                PinRole::RotaryPulse => self.rotary_pulse.is_high(),
                PinRole::RotaryRead => self.rotary_read.is_high(),
            }
        }
    }

    impl PiGpioPort {
        /// Open the configured BCM pins and start the debounced polling loop.
        ///
        /// # Errors
        ///
        /// Returns [`GpioError::Setup`] if no tokio runtime is available for the
        /// polling task or if the GPIO peripheral or any configured pin cannot
        /// be opened.
        pub fn new(config: GpioConfig) -> Result<Self, GpioError> {
            let handle = Handle::try_current().map_err(|err| {
                GpioError::Setup(
                    format!("tokio runtime required for gpio polling task: {err}").into(),
                )
            })?;
            let gpio = Gpio::new()
                .map_err(|err| GpioError::Setup(format!("failed to open gpio: {err}").into()))?;
            let pins = PiPins {
                hook: open_input(&gpio, &config, PinRole::Hook)?,
                rotary_pulse: open_input(&gpio, &config, PinRole::RotaryPulse)?,
                rotary_read: open_input(&gpio, &config, PinRole::RotaryRead)?,
                _gpio: gpio,
            };

            // Seed the shared snapshot levels from the current pin state so a
            // `snapshot` issued right after construction reflects reality before
            // the first poll tick fires.
            let levels = Arc::new([
                AtomicBool::new(logical_level(&pins, &config, PinRole::Hook)),
                AtomicBool::new(logical_level(&pins, &config, PinRole::RotaryPulse)),
                AtomicBool::new(logical_level(&pins, &config, PinRole::RotaryRead)),
            ]);

            let (tx, rx) = mpsc::channel(usize::from(config.channel_capacity).max(1));
            let started_at = Instant::now();
            let debounce = Duration::from_millis(config.debounce_ms);

            info!(
                hook_bcm = config.bcm_for(PinRole::Hook),
                rotary_pulse_bcm = config.bcm_for(PinRole::RotaryPulse),
                rotary_read_bcm = config.bcm_for(PinRole::RotaryRead),
                debounce_ms = config.debounce_ms,
                poll_interval_ms = POLL_INTERVAL.as_millis(),
                pull = ?config.pull,
                "configured raspberry pi gpio inputs (polling)"
            );

            let poll_task = handle.spawn(poll_edges(
                pins,
                config,
                Arc::clone(&levels),
                tx,
                debounce,
                started_at,
            ));

            Ok(Self {
                rx,
                levels,
                poll_task,
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
                .ok_or_else(|| GpioError::Stream("gpio poll task stopped".into()))
        }

        async fn snapshot(&self, role: PinRole) -> Result<bool, GpioError> {
            Ok(self.levels[role_index(role)].load(Ordering::Relaxed))
        }
    }

    impl Drop for PiGpioPort {
        fn drop(&mut self) {
            // Aborting the poll task drops the owned `InputPin`s, releasing the
            // pins cleanly. Unlike the cdev interrupt path there is no line
            // request to clear, so there is nothing that can fail here.
            self.poll_task.abort();
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

    fn logical_level(pins: &PiPins, config: &GpioConfig, role: PinRole) -> bool {
        apply_invert(pins.physical_high(role), config.inverted(role))
    }

    /// Poll every input at [`POLL_INTERVAL`], forwarding a [`GpioEdge`] whenever
    /// a pin's logical level changes and then stays stable for the configured
    /// debounce window. Runs until the receiver is dropped (or the task is
    /// aborted when the [`PiGpioPort`] is dropped).
    async fn poll_edges(
        pins: PiPins,
        config: GpioConfig,
        levels: Arc<[AtomicBool; 3]>,
        tx: mpsc::Sender<GpioEdge>,
        debounce: Duration,
        started_at: Instant,
    ) {
        // Per-role debounce state: the last confirmed logical level and, when a
        // differing reading is seen, the candidate level and when it first
        // appeared. `confirmed` is seeded from the live pins; the shared
        // `levels` (read by `snapshot`) instead mirrors the most recent raw
        // sample so callers always see the current level, not the debounced one.
        let mut confirmed = [
            logical_level(&pins, &config, PinRole::Hook),
            logical_level(&pins, &config, PinRole::RotaryPulse),
            logical_level(&pins, &config, PinRole::RotaryRead),
        ];
        let mut pending: [Option<(bool, Instant)>; 3] = [None, None, None];

        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            let now = Instant::now();

            for role in ROLES {
                let idx = role_index(role);
                let raw = apply_invert(pins.physical_high(role), config.inverted(role));

                // Publish every raw sample so `snapshot` reflects the current
                // level regardless of the debounce state.
                levels[idx].store(raw, Ordering::Relaxed);

                if raw == confirmed[idx] {
                    // Bounced back to the confirmed level; cancel any candidate.
                    pending[idx] = None;
                    continue;
                }

                match pending[idx] {
                    Some((level, since)) if level == raw => {
                        if now.duration_since(since) >= debounce {
                            confirmed[idx] = raw;
                            pending[idx] = None;
                            forward_edge(&tx, role, raw, started_at);
                        }
                    }
                    _ => pending[idx] = Some((raw, now)),
                }
            }

            // Never block the sampler on delivery (see `forward_edge`); instead
            // stop once the consumer is gone so the task doesn't spin forever.
            if tx.is_closed() {
                debug!("gpio edge receiver dropped; stopping poll loop");
                return;
            }
        }
    }

    /// Deliver a debounced edge without ever blocking the sampling loop.
    ///
    /// Awaiting a bounded `send` would pause sampling of *all* pins whenever the
    /// queue is full (its capacity can be as low as 1), silently losing later
    /// transitions. A non-blocking `try_send` keeps sampling alive; a full queue
    /// drops the edge and bumps `booth_gpio_edges_dropped_total` instead.
    fn forward_edge(tx: &mpsc::Sender<GpioEdge>, role: PinRole, level: bool, started_at: Instant) {
        let edge = GpioEdge {
            role,
            level,
            at_monotonic_ns: monotonic_ns(started_at.elapsed()),
        };

        match tx.try_send(edge) {
            Ok(()) => debug!(?role, level, "forwarded debounced gpio edge"),
            Err(mpsc::error::TrySendError::Full(_)) => {
                metrics::counter!("booth_gpio_edges_dropped_total", "role" => role_label(role))
                    .increment(1);
                warn!(?role, "gpio edge queue full; dropping edge");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                debug!(?role, "gpio edge receiver dropped");
            }
        }
    }

    const fn role_label(role: PinRole) -> &'static str {
        match role {
            PinRole::Hook => "Hook",
            PinRole::RotaryPulse => "RotaryPulse",
            PinRole::RotaryRead => "RotaryRead",
        }
    }

    fn apply_invert(physical_high: bool, invert: bool) -> bool {
        if invert {
            !physical_high
        } else {
            physical_high
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
