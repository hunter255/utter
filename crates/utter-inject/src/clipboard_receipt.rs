//! Pure policy for deciding when a promised clipboard value is safe to
//! replace. Kept free of AppKit types so timing and ownership edge cases are
//! deterministic unit tests rather than sleeps against the real pasteboard.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReceiptDecision {
    Pending,
    ReadConfirmed,
    OwnershipLost,
    TimedOut,
}

pub(crate) struct ReceiptPolicy {
    last_count: usize,
    last_read_at: Option<Duration>,
    quiet_period: Duration,
    timeout: Duration,
}

impl ReceiptPolicy {
    pub(crate) fn new(baseline: usize, quiet_period: Duration, timeout: Duration) -> Self {
        Self {
            last_count: baseline,
            last_read_at: None,
            quiet_period,
            timeout,
        }
    }

    pub(crate) fn observe(
        &mut self,
        elapsed: Duration,
        read_count: usize,
        still_owner: bool,
    ) -> ReceiptDecision {
        if !still_owner {
            return ReceiptDecision::OwnershipLost;
        }

        if read_count > self.last_count {
            self.last_count = read_count;
            self.last_read_at = Some(elapsed);
        }

        if self
            .last_read_at
            .is_some_and(|last| elapsed.saturating_sub(last) >= self.quiet_period)
        {
            return ReceiptDecision::ReadConfirmed;
        }

        if elapsed >= self.timeout {
            return ReceiptDecision::TimedOut;
        }

        ReceiptDecision::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUIET: Duration = Duration::from_millis(25);
    const TIMEOUT: Duration = Duration::from_millis(750);

    #[test]
    fn a_read_before_arming_is_not_a_receipt() {
        let mut policy = ReceiptPolicy::new(1, QUIET, TIMEOUT);
        assert_eq!(
            policy.observe(Duration::from_millis(100), 1, true),
            ReceiptDecision::Pending
        );
    }

    #[test]
    fn a_post_arm_read_is_confirmed_after_the_quiet_period() {
        let mut policy = ReceiptPolicy::new(0, QUIET, TIMEOUT);
        assert_eq!(
            policy.observe(Duration::from_millis(10), 1, true),
            ReceiptDecision::Pending
        );
        assert_eq!(
            policy.observe(Duration::from_millis(34), 1, true),
            ReceiptDecision::Pending
        );
        assert_eq!(
            policy.observe(Duration::from_millis(35), 1, true),
            ReceiptDecision::ReadConfirmed
        );
    }

    #[test]
    fn another_read_restarts_the_quiet_period() {
        let mut policy = ReceiptPolicy::new(0, QUIET, TIMEOUT);
        assert_eq!(
            policy.observe(Duration::from_millis(10), 1, true),
            ReceiptDecision::Pending
        );
        assert_eq!(
            policy.observe(Duration::from_millis(30), 2, true),
            ReceiptDecision::Pending
        );
        assert_eq!(
            policy.observe(Duration::from_millis(55), 2, true),
            ReceiptDecision::ReadConfirmed
        );
    }

    #[test]
    fn ownership_loss_always_forbids_restore() {
        let mut policy = ReceiptPolicy::new(0, QUIET, TIMEOUT);
        assert_eq!(
            policy.observe(Duration::from_millis(10), 1, false),
            ReceiptDecision::OwnershipLost
        );
    }

    #[test]
    fn no_read_finishes_at_the_total_timeout() {
        let mut policy = ReceiptPolicy::new(0, QUIET, TIMEOUT);
        assert_eq!(
            policy.observe(TIMEOUT - Duration::from_millis(1), 0, true),
            ReceiptDecision::Pending
        );
        assert_eq!(policy.observe(TIMEOUT, 0, true), ReceiptDecision::TimedOut);
    }
}
