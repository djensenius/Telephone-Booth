//! RGB status-LED adapter.
//!
//! The reference hardware (an Adafruit 3350 pushbutton ring) shares a single
//! current limit across its red / green / blue cathodes, so only one channel
//! can ever be lit — see `docs/adr/0009-status-led-power-button.md`. This
//! adapter therefore drives **at most one** channel at a time and holds the
//! other two at their inactive level. Brightness, pulsing, blinking and fading
//! are rendered with per-channel software PWM (the Pi's hardware PWM channels
//! collide with the I2S audio HAT).
//!
//! Real GPIO is gated behind `#[cfg(all(feature = "pi", target_os = "linux"))]`;
//! every other target uses a stub that keeps the crate type-checking on macOS.
//! [`NoopStatusLed`] is always available for booths that leave the LED disabled.

use async_trait::async_trait;
use booth_hal::{LedColour, LedError, LedPattern, StatusLed};

use crate::StatusLedConfig;

/// A [`StatusLed`] that ignores every request. Used when the status LED is
/// disabled so the runtime can call the port unconditionally.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopStatusLed;

#[async_trait]
impl StatusLed for NoopStatusLed {
    async fn set(&self, _colour: LedColour, _pattern: LedPattern) -> Result<(), LedError> {
        Ok(())
    }
}

/// Software-PWM frequency used for brightness / pulse shaping on the active
/// channel. Fast enough to be flicker-free, slow enough to be gentle on the
/// scheduler.
#[cfg(all(feature = "pi", target_os = "linux"))]
const PWM_FREQUENCY_HZ: f64 = 200.0;

/// How often the animation task recomputes the active channel's duty cycle.
#[cfg(all(feature = "pi", target_os = "linux"))]
const ANIMATION_TICK: std::time::Duration = std::time::Duration::from_millis(25);

/// Compute the lit fraction (`0.0..=1.0`) of the active channel for `pattern`
/// at elapsed time `t`, scaled by the global `ceiling`. Returns `None` once a
/// one-shot pattern (fade) has finished, signalling the caller to stop.
#[cfg(all(feature = "pi", target_os = "linux"))]
fn lit_fraction(pattern: LedPattern, t: std::time::Duration, ceiling: f64) -> Option<f64> {
    // `as_secs_f64() * 1000.0` avoids a lossy `u128 -> f64` cast from `as_millis`.
    let ms = t.as_secs_f64() * 1000.0;
    let frac = match pattern {
        LedPattern::Steady { brightness } => f64::from(brightness) / 255.0,
        LedPattern::Pulse { period_ms } => {
            let period = f64::from(period_ms).max(1.0);
            let phase = (ms % period) / period; // 0..1
            1.0 - 2.0f64.mul_add(phase, -1.0).abs() // triangle 0→1→0
        }
        LedPattern::Blink { period_ms } => {
            let period = f64::from(period_ms).max(1.0);
            if (ms % period) < period / 2.0 {
                1.0
            } else {
                0.0
            }
        }
        LedPattern::Fade { duration_ms } => {
            let duration = f64::from(duration_ms).max(1.0);
            if ms >= duration {
                return None;
            }
            1.0 - (ms / duration)
        }
    };
    Some((frac * ceiling).clamp(0.0, 1.0))
}

#[cfg(all(feature = "pi", target_os = "linux"))]
mod imp {
    use std::sync::Arc;

    use rppal::gpio::{Gpio, OutputPin};
    use tokio::sync::Mutex;
    use tokio::task::JoinHandle;
    use tracing::{info, warn};

    use super::{
        ANIMATION_TICK, LedColour, LedError, LedPattern, PWM_FREQUENCY_HZ, StatusLed,
        StatusLedConfig, async_trait, lit_fraction,
    };

    struct Channels {
        red: OutputPin,
        green: OutputPin,
        blue: OutputPin,
        active_low: bool,
    }

    impl Channels {
        /// Drive `pin` to its inactive (unlit) level.
        fn off_pin(pin: &mut OutputPin, active_low: bool) {
            pin.clear_pwm().ok();
            if active_low {
                pin.set_high();
            } else {
                pin.set_low();
            }
        }

        /// Turn every channel off.
        fn all_off(&mut self) {
            let active_low = self.active_low;
            Self::off_pin(&mut self.red, active_low);
            Self::off_pin(&mut self.green, active_low);
            Self::off_pin(&mut self.blue, active_low);
        }

        /// Turn off the two channels that are not `colour` and return the one
        /// that is (if any). This is the single-channel guarantee in code:
        /// the other two are always driven to their inactive level first.
        fn select(&mut self, colour: LedColour) -> Option<&mut OutputPin> {
            let active_low = self.active_low;
            match colour {
                LedColour::Off => {
                    self.all_off();
                    None
                }
                LedColour::Red => {
                    Self::off_pin(&mut self.green, active_low);
                    Self::off_pin(&mut self.blue, active_low);
                    Some(&mut self.red)
                }
                LedColour::Green => {
                    Self::off_pin(&mut self.red, active_low);
                    Self::off_pin(&mut self.blue, active_low);
                    Some(&mut self.green)
                }
                LedColour::Blue => {
                    Self::off_pin(&mut self.red, active_low);
                    Self::off_pin(&mut self.green, active_low);
                    Some(&mut self.blue)
                }
            }
        }

        /// Apply a lit fraction to the pin for `colour`.
        fn apply(&mut self, colour: LedColour, lit: f64) -> Result<(), LedError> {
            let active_low = self.active_low;
            let Some(pin) = self.select(colour) else {
                return Ok(());
            };
            if lit <= 0.0 {
                Self::off_pin(pin, active_low);
                return Ok(());
            }
            // Active-low sinks the cathode: driving the pin low lights it, so
            // the PWM high fraction is the inverse of the lit fraction.
            let duty = if active_low { 1.0 - lit } else { lit };
            pin.set_pwm_frequency(PWM_FREQUENCY_HZ, duty)
                .map_err(|err| LedError::Write(format!("set pwm: {err}").into()))
        }
    }

    /// Raspberry Pi RGB status-LED adapter (software PWM over `rppal`).
    pub struct PiStatusLed {
        channels: Arc<Mutex<Channels>>,
        animation: Arc<Mutex<Option<JoinHandle<()>>>>,
        ceiling: f64,
    }

    impl PiStatusLed {
        /// Open the three cathode pins and hold them off.
        ///
        /// # Errors
        ///
        /// Returns [`LedError::Setup`] if the GPIO peripheral or any configured
        /// pin cannot be opened.
        pub fn new(config: &StatusLedConfig) -> Result<Self, LedError> {
            let gpio =
                Gpio::new().map_err(|err| LedError::Setup(format!("open gpio: {err}").into()))?;
            let open = |bcm: u8, name: &str| -> Result<OutputPin, LedError> {
                gpio.get(bcm)
                    .map(rppal::gpio::Pin::into_output)
                    .map_err(|err| LedError::Setup(format!("open {name} BCM {bcm}: {err}").into()))
            };
            let mut channels = Channels {
                red: open(config.red, "led red")?,
                green: open(config.green, "led green")?,
                blue: open(config.blue, "led blue")?,
                active_low: config.active_low,
            };
            channels.all_off();
            info!(
                red_bcm = config.red,
                green_bcm = config.green,
                blue_bcm = config.blue,
                active_low = config.active_low,
                "configured raspberry pi status led"
            );
            Ok(Self {
                channels: Arc::new(Mutex::new(channels)),
                animation: Arc::new(Mutex::new(None)),
                ceiling: f64::from(config.brightness_clamped()),
            })
        }
    }

    #[async_trait]
    impl StatusLed for PiStatusLed {
        async fn set(&self, colour: LedColour, pattern: LedPattern) -> Result<(), LedError> {
            // Replace any running animation so only one task drives the pins.
            let mut anim = self.animation.lock().await;
            if let Some(handle) = anim.take() {
                handle.abort();
            }

            if colour == LedColour::Off {
                self.channels.lock().await.all_off();
                return Ok(());
            }

            // Steady output needs no animation task: set the duty once.
            if let LedPattern::Steady { brightness } = pattern {
                let lit = (f64::from(brightness) / 255.0 * self.ceiling).clamp(0.0, 1.0);
                self.channels.lock().await.apply(colour, lit)?;
                return Ok(());
            }

            let channels = Arc::clone(&self.channels);
            let ceiling = self.ceiling;
            *anim = Some(tokio::spawn(async move {
                let start = std::time::Instant::now();
                let mut ticker = tokio::time::interval(ANIMATION_TICK);
                loop {
                    ticker.tick().await;
                    let Some(lit) = lit_fraction(pattern, start.elapsed(), ceiling) else {
                        // One-shot pattern finished: leave the channel off.
                        // Bound to a local so the guard drops before logging.
                        let parked = channels.lock().await.apply(colour, 0.0);
                        if let Err(err) = parked {
                            warn!(%err, %colour, %pattern, "failed to park status LED channel");
                        }
                        break;
                    };
                    // The runtime has already reported this indication as
                    // active, so a mid-animation PWM failure must not be
                    // swallowed: log it before giving up on the animation.
                    let written = channels.lock().await.apply(colour, lit);
                    if let Err(err) = written {
                        warn!(
                            %err, %colour, %pattern,
                            "status LED animation stopped after a write failure"
                        );
                        break;
                    }
                }
            }));
            Ok(())
        }
    }

    impl Drop for PiStatusLed {
        fn drop(&mut self) {
            if let Ok(mut anim) = self.animation.try_lock()
                && let Some(handle) = anim.take()
            {
                handle.abort();
            }
            if let Ok(mut channels) = self.channels.try_lock() {
                channels.all_off();
            }
        }
    }
}

#[cfg(not(all(feature = "pi", target_os = "linux")))]
mod imp {
    use super::{LedColour, LedError, LedPattern, StatusLed, StatusLedConfig, async_trait};

    /// Stub status-LED adapter used when the `pi` feature is disabled or the
    /// target is not Linux (rppal is Linux-only). Constructs successfully so
    /// the runtime can build adapters on any host, but reports `Unsupported`
    /// if actually driven.
    pub struct PiStatusLed;

    impl PiStatusLed {
        /// Construct the stub adapter.
        ///
        /// # Errors
        ///
        /// Never fails; the signature mirrors the real adapter.
        pub fn new(_config: &StatusLedConfig) -> Result<Self, LedError> {
            Ok(Self)
        }
    }

    #[async_trait]
    impl StatusLed for PiStatusLed {
        async fn set(&self, _colour: LedColour, _pattern: LedPattern) -> Result<(), LedError> {
            Err(LedError::Unsupported(
                "booth-pi status led requires the `pi` feature on a Linux target".into(),
            ))
        }
    }
}

pub use imp::PiStatusLed;
