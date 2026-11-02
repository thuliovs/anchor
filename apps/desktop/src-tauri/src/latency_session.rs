#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleDecision {
    Continue,
    Complete,
    FinalSyncTimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptDecision {
    AbortBeforeFirstSample,
    FinishInterrupted,
}

#[derive(Debug)]
pub struct LatencySessionLifecycle {
    duration_us: u64,
    final_sync_timeout_us: u64,
    first_accepted_us: Option<u64>,
    final_sync_deadline_us: Option<u64>,
    interrupted: bool,
}

impl LatencySessionLifecycle {
    pub fn new(duration_us: u64, final_sync_timeout_us: u64) -> Self {
        Self {
            duration_us,
            final_sync_timeout_us,
            first_accepted_us: None,
            final_sync_deadline_us: None,
            interrupted: false,
        }
    }

    pub fn accept_sample(&mut self, now_us: u64) -> bool {
        if self.first_accepted_us.is_none() {
            self.first_accepted_us = Some(now_us);
            true
        } else {
            false
        }
    }

    pub fn has_samples(&self) -> bool {
        self.first_accepted_us.is_some()
    }

    pub fn first_accepted_us(&self) -> Option<u64> {
        self.first_accepted_us
    }

    pub fn window_done(&self, now_us: u64) -> bool {
        self.first_accepted_us
            .is_some_and(|first| now_us >= first.saturating_add(self.duration_us))
    }

    pub fn final_sync_deadline_us(&self) -> Option<u64> {
        self.final_sync_deadline_us
    }

    pub fn interrupted(&self) -> bool {
        self.interrupted
    }

    pub fn interrupt(&mut self) -> InterruptDecision {
        if self.has_samples() {
            self.interrupted = true;
            InterruptDecision::FinishInterrupted
        } else {
            InterruptDecision::AbortBeforeFirstSample
        }
    }

    pub fn poll(
        &mut self,
        now_us: u64,
        final_probe_after_window: bool,
        runtime_drained: bool,
        last_anchor_midpoint_us: Option<f64>,
    ) -> LifecycleDecision {
        if !self.window_done(now_us) {
            return LifecycleDecision::Continue;
        }
        let deadline = *self
            .final_sync_deadline_us
            .get_or_insert_with(|| now_us.saturating_add(self.final_sync_timeout_us));
        let window_end = self
            .first_accepted_us
            .unwrap_or_default()
            .saturating_add(self.duration_us) as f64;
        if final_probe_after_window
            && runtime_drained
            && last_anchor_midpoint_us.is_some_and(|anchor| anchor >= window_end)
        {
            LifecycleDecision::Complete
        } else if now_us >= deadline {
            LifecycleDecision::FinalSyncTimedOut
        } else {
            LifecycleDecision::Continue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_c_before_and_during_measurement_are_distinct() {
        let mut before = LatencySessionLifecycle::new(60, 5);
        assert_eq!(
            before.interrupt(),
            InterruptDecision::AbortBeforeFirstSample
        );
        assert!(!before.interrupted());

        let mut during = LatencySessionLifecycle::new(60, 5);
        assert!(during.accept_sample(10));
        assert_eq!(during.interrupt(), InterruptDecision::FinishInterrupted);
        assert!(during.interrupted());
    }

    #[test]
    fn final_timeout_is_deterministic_and_starts_after_window() {
        let mut lifecycle = LatencySessionLifecycle::new(60, 5);
        lifecycle.accept_sample(10);
        assert_eq!(
            lifecycle.poll(69, false, false, None),
            LifecycleDecision::Continue
        );
        assert_eq!(lifecycle.final_sync_deadline_us(), None);
        assert_eq!(
            lifecycle.poll(70, false, false, None),
            LifecycleDecision::Continue
        );
        assert_eq!(lifecycle.final_sync_deadline_us(), Some(75));
        assert_eq!(
            lifecycle.poll(74, false, false, None),
            LifecycleDecision::Continue
        );
        assert_eq!(
            lifecycle.poll(75, false, false, None),
            LifecycleDecision::FinalSyncTimedOut
        );
    }

    #[test]
    fn completion_requires_final_probe_runtime_drain_and_covering_anchor() {
        let mut lifecycle = LatencySessionLifecycle::new(60, 5);
        lifecycle.accept_sample(10);
        for (probe, drained, anchor) in [
            (false, true, Some(70.0)),
            (true, false, Some(70.0)),
            (true, true, Some(69.0)),
            (true, true, None),
        ] {
            assert_eq!(
                lifecycle.poll(70, probe, drained, anchor),
                LifecycleDecision::Continue
            );
        }
        assert_eq!(
            lifecycle.poll(70, true, true, Some(70.0)),
            LifecycleDecision::Complete
        );
    }
}
