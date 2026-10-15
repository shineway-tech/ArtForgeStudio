use std::time::{Duration, SystemTime};

pub(super) struct AnnouncementSchedule {
    interval: Duration,
    next_due: Option<SystemTime>,
    in_flight: bool,
}

impl AnnouncementSchedule {
    pub(super) fn new(device_id: &str) -> Self {
        let offset = device_id.bytes().fold(0u64, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(byte as u64)
        }) % 61;
        Self {
            interval: Duration::from_secs(1800 + offset),
            next_due: None,
            in_flight: false,
        }
    }
    pub(super) fn due(&self, now: SystemTime) -> bool {
        !self.in_flight && self.next_due.map_or(true, |deadline| now >= deadline)
    }
    pub(super) fn in_flight(&self) -> bool {
        self.in_flight
    }
    pub(super) fn started(&mut self, now: SystemTime) {
        self.in_flight = true;
        self.next_due = now.checked_add(self.interval);
    }
    pub(super) fn finished(&mut self, success: bool, now: SystemTime) {
        self.in_flight = false;
        if !success {
            self.next_due = now.checked_add(Duration::from_secs(60));
        }
    }
    pub(super) fn reset(&mut self) {
        self.next_due = None;
        self.in_flight = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn time(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn first_login_is_immediate_and_inflight_polls_cannot_overlap() {
        let mut schedule = AnnouncementSchedule::new("");
        assert!(schedule.due(time(0)));
        schedule.started(time(0));
        assert!(!schedule.due(time(9999)));
        schedule.finished(true, time(1));
        assert!(!schedule.due(time(1799)));
        assert!(schedule.due(time(1800)));
    }
    #[test]
    fn elapsed_wall_time_catches_up_after_sleep_and_reset_starts_new_account() {
        let mut schedule = AnnouncementSchedule::new("");
        schedule.started(time(100));
        schedule.finished(true, time(101));
        assert!(schedule.due(time(7200)));
        schedule.reset();
        assert!(schedule.due(time(102)));
    }
    #[test]
    fn failures_retry_without_busy_loop_and_devices_stagger_within_one_minute() {
        let mut schedule = AnnouncementSchedule::new("device-a");
        assert!((1800..=1860).contains(&schedule.interval.as_secs()));
        schedule.started(time(100));
        schedule.finished(false, time(103));
        assert!(!schedule.due(time(162)));
        assert!(schedule.due(time(163)));
    }
}
