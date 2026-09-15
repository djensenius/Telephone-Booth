//! Bounded, asynchronous reconciliation of operator-controlled exhibitions.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use booth_hal::{
    BoothStatus, EventBatchAck, InstallationState, OperatorClient, OperatorError, OperatorMessage,
    OperatorQuestion, QuestionId, SystemSnapshot, UploadMetadata, UploadSlot,
};
use booth_telemetry::TelemetryBus;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use tracing::{debug, info};

use super::FetchGeneration;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const ACTIVE_LEASE: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Unknown,
    Legacy,
    Active,
    BetweenExhibitions,
}

#[derive(Clone, Copy)]
pub struct Observation {
    mode: Mode,
    checked: Instant,
    revision: u64,
}

/// Shared admission decision; network health never manufactures inactivity.
#[derive(Clone)]
pub struct InstallationGate {
    state: Arc<watch::Sender<Observation>>,
    pub(crate) fetch_generation: FetchGeneration,
}

impl InstallationGate {
    pub(crate) fn new(reconcile: bool) -> Self {
        let (state, _) = watch::channel(Observation {
            mode: if reconcile {
                Mode::Unknown
            } else {
                Mode::Legacy
            },
            checked: Instant::now(),
            revision: 0,
        });
        Self {
            state: Arc::new(state),
            fetch_generation: FetchGeneration::default(),
        }
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Observation> {
        self.state.subscribe()
    }

    pub(crate) fn accepting_calls(&self) -> bool {
        self.check().is_ok()
    }

    fn check(&self) -> Result<(), OperatorError> {
        let state = *self.state.borrow();
        match state.mode {
            Mode::Legacy => Ok(()),
            Mode::Active if state.checked.elapsed() < ACTIVE_LEASE => Ok(()),
            Mode::BetweenExhibitions => Err(OperatorError::InstallationInactive(
                "waiting for an operator to start the next exhibition".into(),
            )),
            Mode::Unknown | Mode::Active => Err(OperatorError::Transport(
                "installation lifecycle is unknown or stale; waiting for GET /v1/status".into(),
            )),
        }
    }

    fn observe<T>(&self, result: Result<T, OperatorError>) -> Result<T, OperatorError> {
        if matches!(result, Err(OperatorError::InstallationInactive(_))) {
            self.state.send_if_modified(|state| {
                // Even a repeated conflict invalidates an older in-flight GET.
                state.revision = state.revision.wrapping_add(1);
                if state.mode != Mode::BetweenExhibitions {
                    info!("installation between exhibitions; pausing new calls and uploads");
                    self.fetch_generation.invalidate();
                    state.mode = Mode::BetweenExhibitions;
                    return true;
                }
                false
            });
        }
        result
    }

    fn reconcile(&self, revision: u64, result: &Result<Option<InstallationState>, OperatorError>) {
        self.state.send_if_modified(|state| {
            if state.revision != revision {
                return false;
            }
            let next = match result {
                Ok(Some(InstallationState::Active)) => Mode::Active,
                Ok(Some(InstallationState::BetweenExhibitions)) => Mode::BetweenExhibitions,
                Ok(None) => Mode::Legacy,
                Err(_) if state.mode == Mode::Active && state.checked.elapsed() >= ACTIVE_LEASE => {
                    Mode::Unknown
                }
                Err(_) => return false,
            };
            if next != state.mode {
                info!(mode = ?next, "installation admission changed");
                if matches!(next, Mode::Unknown | Mode::BetweenExhibitions) {
                    self.fetch_generation.invalidate();
                }
            }
            state.mode = next;
            state.checked = Instant::now();
            state.revision = state.revision.wrapping_add(1);
            true
        });
    }

    pub(crate) fn spawn_reconciler(
        &self,
        operator: Arc<dyn OperatorClient>,
        bus: TelemetryBus,
    ) -> JoinHandle<()> {
        let gate = self.clone();
        tokio::spawn(async move {
            if !operator.supports_installation_state() {
                return;
            }
            let mut interval = tokio::time::interval(POLL_INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                interval.tick().await;
                let revision = gate.state.borrow().revision;
                let result = tokio::time::timeout(PROBE_TIMEOUT, operator.installation_state())
                    .await
                    .unwrap_or_else(|_| {
                        Err(OperatorError::Transport(
                            "installation status check timed out".into(),
                        ))
                    });
                if let Err(ref err) = result {
                    super::publish_operator_error(&bus, "installation_state", err);
                }
                gate.reconcile(revision, &result);
            }
        })
    }
}

/// Central gate also covers independent heartbeat and event-forwarder tasks.
pub struct ExhibitionOperator {
    pub(crate) inner: Arc<dyn OperatorClient>,
    pub(crate) gate: InstallationGate,
}

#[cfg(all(test, feature = "mock"))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use booth_mock::MockOperatorClient;

    #[tokio::test(start_paused = true)]
    async fn startup_stale_active_and_legacy_have_distinct_policies() {
        let gate = InstallationGate::new(true);
        assert!(matches!(gate.check(), Err(OperatorError::Transport(_))));
        gate.reconcile(0, &Ok(Some(InstallationState::Active)));
        assert!(gate.accepting_calls());
        tokio::time::advance(ACTIVE_LEASE).await;
        assert!(matches!(gate.check(), Err(OperatorError::Transport(_))));
        let revision = gate.state.borrow().revision;
        gate.reconcile(revision, &Ok(None));
        tokio::time::advance(ACTIVE_LEASE * 10).await;
        assert!(
            gate.accepting_calls(),
            "missing field preserves old behavior"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn confirmed_inactive_is_sticky_and_invalidates_older_active_probes() {
        let gate = InstallationGate::new(true);
        gate.reconcile(0, &Ok(Some(InstallationState::Active)));
        let revision = gate.state.borrow().revision;
        let fetch = gate.fetch_generation.current();
        let _: Result<(), _> = gate.observe(Err(OperatorError::InstallationInactive("gap".into())));
        assert!(!fetch.is_current());
        gate.reconcile(revision, &Ok(Some(InstallationState::Active)));
        assert!(
            !gate.accepting_calls(),
            "an older GET cannot undo a newer 409"
        );
        let revision = gate.state.borrow().revision;
        gate.reconcile(
            revision,
            &Err(OperatorError::Unauthorized("bad token".into())),
        );
        tokio::time::advance(ACTIVE_LEASE * 10).await;
        assert!(matches!(
            gate.check(),
            Err(OperatorError::InstallationInactive(_))
        ));
        gate.reconcile(revision, &Ok(Some(InstallationState::Active)));
        assert!(gate.accepting_calls());
    }

    #[tokio::test]
    async fn inactive_gate_preserves_events_and_allows_system_telemetry() {
        let inner = Arc::new(MockOperatorClient::new());
        let gate = InstallationGate::new(true);
        gate.reconcile(0, &Ok(Some(InstallationState::BetweenExhibitions)));
        let operator = ExhibitionOperator {
            inner: inner.clone(),
            gate,
        };
        assert!(matches!(
            operator.random_question().await,
            Err(OperatorError::InstallationInactive(_))
        ));
        assert!(matches!(
            operator.random_message().await,
            Err(OperatorError::InstallationInactive(_))
        ));
        assert!(matches!(
            operator.random_instruction().await,
            Err(OperatorError::InstallationInactive(_))
        ));
        assert!(matches!(
            operator.put_status(BoothStatus::Idle).await,
            Err(OperatorError::InstallationInactive(_))
        ));
        assert!(matches!(
            operator.push_events_json(r#"{"events":[]}"#).await,
            Err(OperatorError::InstallationInactive(_))
        ));
        operator
            .put_system_snapshot("booth", "test", &SystemSnapshot::default())
            .await
            .unwrap();
        let state = inner.state();
        let state = state.lock().await;
        assert!(state.statuses.is_empty());
        assert!(state.event_batches.is_empty());
        assert_eq!(state.system_snapshots.len(), 1);
    }
}

#[async_trait]
impl OperatorClient for ExhibitionOperator {
    fn supports_installation_state(&self) -> bool {
        self.inner.supports_installation_state()
    }

    async fn installation_state(&self) -> Result<Option<InstallationState>, OperatorError> {
        self.inner.installation_state().await
    }

    async fn random_question(&self) -> Result<OperatorQuestion, OperatorError> {
        self.gate.check()?;
        let value = self.gate.observe(self.inner.random_question().await)?;
        self.gate.check()?;
        Ok(value)
    }

    async fn random_question_with_draw_id(
        &self,
        draw_id: &str,
    ) -> Result<OperatorQuestion, OperatorError> {
        self.gate.check()?;
        let value = self
            .gate
            .observe(self.inner.random_question_with_draw_id(draw_id).await)?;
        self.gate.check()?;
        Ok(value)
    }

    async fn random_message(&self) -> Result<OperatorMessage, OperatorError> {
        self.gate.check()?;
        let value = self.gate.observe(self.inner.random_message().await)?;
        self.gate.check()?;
        Ok(value)
    }

    async fn instructions(&self) -> Result<OperatorMessage, OperatorError> {
        self.random_instruction().await
    }

    async fn random_instruction(&self) -> Result<OperatorMessage, OperatorError> {
        self.gate.check()?;
        let value = self.gate.observe(self.inner.random_instruction().await)?;
        self.gate.check()?;
        Ok(value)
    }

    async fn init_upload(
        &self,
        question_id: Option<&QuestionId>,
        metadata: &UploadMetadata,
    ) -> Result<UploadSlot, OperatorError> {
        self.gate.check()?;
        self.gate
            .observe(self.inner.init_upload(question_id, metadata).await)
    }

    async fn put_upload(
        &self,
        slot: &UploadSlot,
        local_path: &str,
        sha256_hex: &str,
    ) -> Result<(), OperatorError> {
        self.gate.check()?;
        self.gate
            .observe(self.inner.put_upload(slot, local_path, sha256_hex).await)
    }

    async fn complete_upload(
        &self,
        slot_id: &str,
        sha256_hex: &str,
        duration_ms: u64,
    ) -> Result<(), OperatorError> {
        self.gate.check()?;
        self.gate.observe(
            self.inner
                .complete_upload(slot_id, sha256_hex, duration_ms)
                .await,
        )
    }

    async fn put_status(&self, status: BoothStatus) -> Result<(), OperatorError> {
        self.gate.check()?;
        self.gate.observe(self.inner.put_status(status).await)
    }

    async fn push_events_json(&self, body: &str) -> Result<EventBatchAck, OperatorError> {
        self.gate.check()?;
        let result = self.gate.observe(self.inner.push_events_json(body).await);
        if matches!(result, Err(OperatorError::InstallationInactive(_))) {
            debug!("retaining exhibition events until manual start");
        }
        result
    }

    async fn put_system_snapshot(
        &self,
        booth_id: &str,
        version: &str,
        snapshot: &SystemSnapshot,
    ) -> Result<(), OperatorError> {
        self.inner
            .put_system_snapshot(booth_id, version, snapshot)
            .await
    }
}
