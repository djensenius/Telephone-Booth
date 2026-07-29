//! In-process telemetry bus for the Telephone Booth phone client.
//!
//! The bus assigns monotonically increasing ids, stores a bounded replay window,
//! and broadcasts every [`booth_hal::TelemetryEvent`] to live subscribers.

#![warn(missing_docs)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::SystemTime;

use booth_hal::TelemetryEvent;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// A telemetry event plus the metadata assigned by [`TelemetryBus`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryRecord {
    /// Monotonic id; clients pass this back as `since` to catch up.
    pub id: u64,
    /// Wall-clock timestamp assigned when the event was published.
    pub ts: SystemTime,
    /// Structured event payload produced by HAL adapters, runtime code, or core effects.
    #[serde(flatten)]
    pub event: TelemetryEvent,
}

/// Fixed-capacity replay store indexed by each record's monotonic id.
#[derive(Debug, Clone)]
pub struct RingBuffer {
    capacity: usize,
    records: VecDeque<TelemetryRecord>,
}

impl RingBuffer {
    /// Create an empty replay buffer that retains at most `capacity` records.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            records: VecDeque::with_capacity(capacity),
        }
    }

    /// Store one record, evicting the oldest record when the buffer is full.
    pub fn push(&mut self, record: TelemetryRecord) {
        if self.capacity == 0 {
            return;
        }
        while self.records.len() >= self.capacity {
            let _dropped = self.records.pop_front();
        }
        self.records.push_back(record);
    }

    /// Return retained records with `id > since_id`, or every retained record when omitted.
    pub fn snapshot_since(&self, since_id: Option<u64>) -> Vec<TelemetryRecord> {
        self.records
            .iter()
            .filter(|record| since_id.is_none_or(|id| record.id > id))
            .cloned()
            .collect()
    }

    /// Return the number of retained records for diagnostics and tests.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Return true when no records are currently retained.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Return the maximum number of records this buffer can retain.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// Broadcast telemetry bus with a bounded replay buffer for reconnect catch-up.
#[derive(Clone)]
pub struct TelemetryBus {
    sender: broadcast::Sender<TelemetryRecord>,
    ring: Arc<RwLock<RingBuffer>>,
    next_id: Arc<AtomicU64>,
    latest_status_led: Arc<RwLock<Option<TelemetryRecord>>>,
}

impl TelemetryBus {
    /// Create a bus whose live channel and replay ring retain up to `capacity` records.
    pub fn new(capacity: usize) -> Self {
        let (sender, _receiver) = broadcast::channel(capacity.max(1));
        Self {
            sender,
            ring: Arc::new(RwLock::new(RingBuffer::new(capacity))),
            next_id: Arc::new(AtomicU64::new(0)),
            latest_status_led: Arc::new(RwLock::new(None)),
        }
    }

    /// Publish one event to the replay ring and all current live subscribers.
    pub fn publish(&self, event: TelemetryEvent) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let record = TelemetryRecord {
            id,
            ts: SystemTime::now(),
            event,
        };
        // The status LED is level-triggered: an unchanged indication is never
        // re-published, so it can outlive the bounded ring by hours. Retain the
        // latest one separately and synchronously, so a debug client that
        // connects (or lags) long after the change still sees the truth.
        if matches!(record.event, TelemetryEvent::StatusLed { .. }) {
            let mut latest = self
                .latest_status_led
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Ids are handed out before this lock, so a concurrent publisher
            // can arrive here out of order. Keep the newest record or the slot
            // could be permanently pinned to a superseded indication.
            if latest.as_ref().is_none_or(|held| record.id > held.id) {
                *latest = Some(record.clone());
            }
        }
        self.write_ring().push(record.clone());
        if self.sender.send(record).is_err() {
            // No active subscribers; the replay ring above remains authoritative.
        }
    }

    /// Latest [`TelemetryEvent::StatusLed`] record, retained independently of
    /// the replay ring.
    ///
    /// Returns `None` only when the LED has never been driven.
    #[must_use]
    pub fn latest_status_led(&self) -> Option<TelemetryRecord> {
        self.latest_status_led
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Subscribe to events published after this call for live debug streaming.
    pub fn subscribe(&self) -> broadcast::Receiver<TelemetryRecord> {
        self.sender.subscribe()
    }

    /// Return retained records newer than `since_id`, or the full replay window when omitted.
    pub fn snapshot_since(&self, since_id: Option<u64>) -> Vec<TelemetryRecord> {
        self.read_ring().snapshot_since(since_id)
    }

    fn read_ring(&self) -> RwLockReadGuard<'_, RingBuffer> {
        self.ring
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_ring(&self) -> RwLockWriteGuard<'_, RingBuffer> {
        self.ring
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::SystemTime;

    use booth_hal::{AudioChannel, AudioLevel, GpioEdge, PinRole, TelemetryEvent};

    use super::{RingBuffer, TelemetryBus, TelemetryRecord};

    fn log_event(message: &str) -> TelemetryEvent {
        TelemetryEvent::Log {
            level: "info".to_string(),
            target: "booth_telemetry::tests".to_string(),
            message: message.to_string(),
        }
    }

    fn record(id: u64) -> TelemetryRecord {
        TelemetryRecord {
            id,
            ts: SystemTime::UNIX_EPOCH,
            event: log_event("record"),
        }
    }

    #[tokio::test]
    async fn publish_subscribe_round_trip() -> Result<(), Box<dyn Error>> {
        let bus = TelemetryBus::new(8);
        let mut stream = bus.subscribe();
        bus.publish(TelemetryEvent::GpioEdge(GpioEdge {
            role: PinRole::Hook,
            level: true,
            at_monotonic_ns: 7,
        }));

        let record = stream.recv().await?;
        assert_eq!(record.id, 1);
        assert!(matches!(
            record.event,
            TelemetryEvent::GpioEdge(GpioEdge {
                role: PinRole::Hook,
                level: true,
                at_monotonic_ns: 7,
            })
        ));
        let json = serde_json::to_string(&record)?;
        assert!(json.contains("\"id\":1"));
        Ok(())
    }

    #[test]
    fn ring_buffer_enforces_capacity_and_evicts_oldest() {
        let mut ring = RingBuffer::new(2);
        ring.push(record(1));
        ring.push(record(2));
        ring.push(record(3));

        let ids: Vec<u64> = ring
            .snapshot_since(None)
            .into_iter()
            .map(|item| item.id)
            .collect();
        assert_eq!(ids, vec![2, 3]);
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.capacity(), 2);
    }

    /// The LED is level-triggered, so its record can be evicted from the
    /// bounded ring long before the indication changes. The retained slot must
    /// outlive eviction or the debug surface would report a stale colour.
    #[test]
    fn latest_status_led_survives_ring_eviction() {
        use booth_hal::{LedColour, LedPattern};

        let bus = TelemetryBus::new(2);
        bus.publish(TelemetryEvent::StatusLed {
            colour: LedColour::Green,
            pattern: LedPattern::Steady { brightness: 255 },
            at_monotonic_ns: 7,
        });
        assert!(bus.latest_status_led().is_some());

        for _ in 0..4 {
            bus.publish(TelemetryEvent::Log {
                level: "info".to_string(),
                target: "booth_telemetry::tests".to_string(),
                message: "filler".to_string(),
            });
        }
        assert!(
            !bus.snapshot_since(None)
                .iter()
                .any(|record| matches!(record.event, TelemetryEvent::StatusLed { .. })),
            "the LED record must have been evicted for this test to mean anything"
        );

        assert!(
            matches!(
                bus.latest_status_led().map(|record| record.event),
                Some(TelemetryEvent::StatusLed {
                    colour: LedColour::Green,
                    at_monotonic_ns: 7,
                    ..
                })
            ),
            "the retained LED record must survive eviction"
        );
    }

    #[test]
    fn snapshot_since_returns_only_newer_monotonic_records() {
        let bus = TelemetryBus::new(8);
        for index in 0..5 {
            bus.publish(log_event(&format!("event-{index}")));
        }

        let snapshot = bus.snapshot_since(Some(2));
        let ids: Vec<u64> = snapshot.into_iter().map(|item| item.id).collect();
        assert_eq!(ids, vec![3, 4, 5]);
    }

    #[tokio::test]
    async fn concurrent_publishers_and_subscribers() -> Result<(), Box<dyn Error>> {
        let bus = TelemetryBus::new(128);
        let publisher_a = bus.clone();
        let publisher_b = bus.clone();
        let mut subscriber = bus.subscribe();

        let publish_a = async move {
            for index in 0..25 {
                publisher_a.publish(log_event(&format!("a-{index}")));
                tokio::task::yield_now().await;
            }
        };
        let publish_b = async move {
            for index in 0_u16..25 {
                publisher_b.publish(TelemetryEvent::AudioLevel(AudioLevel {
                    channel: AudioChannel::Input,
                    peak: f32::from(index),
                    rms: f32::from(index) / 2.0,
                    at_monotonic_ns: u64::from(index),
                }));
                tokio::task::yield_now().await;
            }
        };
        let receive = async move {
            let mut seen = 0_u64;
            while seen < 50 {
                match subscriber.recv().await {
                    Ok(_record) => seen += 1,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        seen += skipped;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            seen
        };

        let ((), (), seen) = tokio::join!(publish_a, publish_b, receive);
        assert_eq!(seen, 50);
        let ids: Vec<u64> = bus
            .snapshot_since(None)
            .into_iter()
            .map(|item| item.id)
            .collect();
        assert_eq!(ids.len(), 50);
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        Ok(())
    }

    #[tokio::test]
    async fn lagging_subscriber_does_not_deadlock_publisher() {
        let bus = TelemetryBus::new(1);
        let mut subscriber = bus.subscribe();

        for index in 0..64 {
            bus.publish(log_event(&format!("event-{index}")));
        }

        assert_eq!(bus.snapshot_since(None).len(), 1);
        match subscriber.recv().await {
            Ok(record) => assert_eq!(record.id, 64),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                assert!(skipped > 0);
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                panic!("publisher should not close while bus is alive");
            }
        }
    }
}
