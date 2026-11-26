use crate::validation::validate_text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    pub windows: u32,
    pub mac: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub counters: Counters,
    pub windows_has_text: bool,
    pub mac_has_text: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    WindowsToMac,
    MacToWindows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Baseline,
    Idle,
    Unsupported,
    Conflict,
    Transfer(Direction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadDecision {
    Invalid,
    Identical,
    Write,
}

#[derive(Debug, Default)]
pub struct SyncEngine {
    baseline: Option<Counters>,
}

impl SyncEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.baseline = None;
    }

    pub fn observe(&mut self, snapshot: Snapshot) -> Decision {
        let current = snapshot.counters;

        let Some(previous) = self.baseline else {
            self.baseline = Some(current);

            return Decision::Baseline;
        };

        let windows_changed = current.windows != previous.windows;

        let mac_changed = current.mac != previous.mac;

        let decision = match (windows_changed, mac_changed) {
            (false, false) => Decision::Idle,

            (true, true) => Decision::Conflict,

            (true, false) if snapshot.windows_has_text => {
                Decision::Transfer(Direction::WindowsToMac)
            }

            (false, true) if snapshot.mac_has_text => Decision::Transfer(Direction::MacToWindows),

            _ => Decision::Unsupported,
        };

        if !matches!(decision, Decision::Transfer(_)) {
            self.baseline = Some(current);
        }

        decision
    }

    pub fn record_skipped(&mut self, observed: Counters) {
        self.baseline = Some(observed);
    }

    pub fn record_mac_write(&mut self, observed: Counters, written_change_count: i64) {
        self.baseline = Some(Counters {
            windows: observed.windows,
            mac: written_change_count,
        });
    }

    pub fn record_windows_write(&mut self, observed: Counters, written_sequence: u32) {
        self.baseline = Some(Counters {
            windows: written_sequence,
            mac: observed.mac,
        });
    }
}

pub fn still_current(expected: Counters, actual: Counters) -> bool {
    expected == actual
}

pub fn inspect_payload(source: &str, destination: Option<&str>) -> PayloadDecision {
    if validate_text(source).is_err() {
        return PayloadDecision::Invalid;
    }

    if let Some(destination) = destination
        && source.as_bytes() == destination.as_bytes()
    {
        return PayloadDecision::Identical;
    }

    PayloadDecision::Write
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::MAX_CLIPBOARD_BYTES;

    fn snapshot(windows: u32, mac: i64) -> Snapshot {
        Snapshot {
            counters: Counters { windows, mac },
            windows_has_text: true,
            mac_has_text: true,
        }
    }

    fn initialized() -> SyncEngine {
        let mut engine = SyncEngine::new();

        assert_eq!(engine.observe(snapshot(10, 20)), Decision::Baseline);

        engine
    }

    #[test]
    fn startup_preserves_both_clipboards() {
        let mut engine = SyncEngine::new();

        assert_eq!(engine.observe(snapshot(100, 200)), Decision::Baseline);

        assert_eq!(engine.observe(snapshot(100, 200)), Decision::Idle);
    }

    #[test]
    fn unchanged_clipboards_are_idle() {
        let mut engine = initialized();

        for _ in 0..100 {
            assert_eq!(engine.observe(snapshot(10, 20)), Decision::Idle);
        }
    }

    #[test]
    fn windows_change_requests_mac_transfer() {
        let mut engine = initialized();

        assert_eq!(
            engine.observe(snapshot(11, 20)),
            Decision::Transfer(Direction::WindowsToMac)
        );
    }

    #[test]
    fn mac_change_requests_windows_transfer() {
        let mut engine = initialized();

        assert_eq!(
            engine.observe(snapshot(10, 21)),
            Decision::Transfer(Direction::MacToWindows)
        );
    }

    #[test]
    fn simultaneous_changes_preserve_both_clipboards() {
        let mut engine = initialized();

        assert_eq!(engine.observe(snapshot(11, 21)), Decision::Conflict);

        assert_eq!(engine.observe(snapshot(11, 21)), Decision::Idle);

        assert_eq!(
            engine.observe(snapshot(12, 21)),
            Decision::Transfer(Direction::WindowsToMac)
        );
    }

    #[test]
    fn unsupported_windows_content_is_ignored() {
        let mut engine = initialized();

        let mut current = snapshot(11, 20);
        current.windows_has_text = false;

        assert_eq!(engine.observe(current), Decision::Unsupported);

        assert_eq!(engine.observe(current), Decision::Idle);
    }

    #[test]
    fn unsupported_mac_content_is_ignored() {
        let mut engine = initialized();

        let mut current = snapshot(10, 21);
        current.mac_has_text = false;

        assert_eq!(engine.observe(current), Decision::Unsupported);

        assert_eq!(engine.observe(current), Decision::Idle);
    }

    #[test]
    fn mac_write_does_not_create_feedback() {
        let mut engine = initialized();

        let current = snapshot(11, 20);

        assert_eq!(
            engine.observe(current),
            Decision::Transfer(Direction::WindowsToMac)
        );

        engine.record_mac_write(current.counters, 21);

        assert_eq!(engine.observe(snapshot(11, 21)), Decision::Idle);
    }

    #[test]
    fn windows_write_does_not_create_feedback() {
        let mut engine = initialized();

        let current = snapshot(10, 21);

        assert_eq!(
            engine.observe(current),
            Decision::Transfer(Direction::MacToWindows)
        );

        engine.record_windows_write(current.counters, 15);

        assert_eq!(engine.observe(snapshot(15, 21)), Decision::Idle);
    }

    #[test]
    fn new_source_change_after_transfer_is_detected() {
        let mut engine = initialized();

        let current = snapshot(11, 20);

        assert_eq!(
            engine.observe(current),
            Decision::Transfer(Direction::WindowsToMac)
        );

        engine.record_mac_write(current.counters, 21);

        assert_eq!(
            engine.observe(snapshot(12, 21)),
            Decision::Transfer(Direction::WindowsToMac)
        );
    }

    #[test]
    fn new_destination_change_after_transfer_is_detected() {
        let mut engine = initialized();

        let current = snapshot(11, 20);

        assert_eq!(
            engine.observe(current),
            Decision::Transfer(Direction::WindowsToMac)
        );

        engine.record_mac_write(current.counters, 21);

        assert_eq!(
            engine.observe(snapshot(11, 22)),
            Decision::Transfer(Direction::MacToWindows)
        );
    }

    #[test]
    fn skipped_transfer_is_not_repeated() {
        let mut engine = initialized();

        let current = snapshot(11, 20);

        assert_eq!(
            engine.observe(current),
            Decision::Transfer(Direction::WindowsToMac)
        );

        engine.record_skipped(current.counters);

        assert_eq!(engine.observe(current), Decision::Idle);
    }

    #[test]
    fn counter_wraparound_is_detected() {
        let mut engine = SyncEngine::new();

        assert_eq!(engine.observe(snapshot(u32::MAX, 20)), Decision::Baseline);

        assert_eq!(
            engine.observe(snapshot(1, 20)),
            Decision::Transfer(Direction::WindowsToMac)
        );
    }

    #[test]
    fn reset_establishes_fresh_baseline() {
        let mut engine = initialized();

        engine.reset();

        assert_eq!(engine.observe(snapshot(500, 600)), Decision::Baseline);

        assert_eq!(engine.observe(snapshot(500, 600)), Decision::Idle);
    }

    #[test]
    fn matching_counters_are_current() {
        let counters = snapshot(10, 20).counters;

        assert!(still_current(counters, counters));
    }

    #[test]
    fn source_or_destination_race_is_detected() {
        let original = snapshot(10, 20).counters;

        assert!(!still_current(original, snapshot(11, 20).counters));

        assert!(!still_current(original, snapshot(10, 21).counters));
    }

    #[test]
    fn identical_payload_is_not_written() {
        let fixture = "Árvíztűrő 🚀\r\n    Code  \n";

        assert_eq!(
            inspect_payload(fixture, Some(fixture)),
            PayloadDecision::Identical
        );
    }

    #[test]
    fn different_payload_is_written() {
        assert_eq!(
            inspect_payload("Hello", Some("World")),
            PayloadDecision::Write
        );
    }

    #[test]
    fn line_endings_are_compared_exactly() {
        assert_eq!(
            inspect_payload("Line\n", Some("Line\r\n")),
            PayloadDecision::Write
        );
    }

    #[test]
    fn unicode_is_compared_byte_for_byte() {
        assert_eq!(
            inspect_payload("\u{00e9}", Some("e\u{0301}")),
            PayloadDecision::Write
        );
    }

    #[test]
    fn text_can_replace_non_text_destination() {
        assert_eq!(inspect_payload("Hello", None), PayloadDecision::Write);
    }

    #[test]
    fn empty_payload_is_rejected() {
        assert_eq!(
            inspect_payload("", Some("Keep me")),
            PayloadDecision::Invalid
        );
    }

    #[test]
    fn embedded_nul_payload_is_rejected() {
        assert_eq!(
            inspect_payload("a\0b", Some("Keep me")),
            PayloadDecision::Invalid
        );
    }

    #[test]
    fn oversized_payload_is_rejected() {
        let oversized = "x".repeat(MAX_CLIPBOARD_BYTES + 1);

        assert_eq!(inspect_payload(&oversized, None), PayloadDecision::Invalid);
    }

    #[test]
    fn maximum_payload_is_accepted() {
        let maximum = "x".repeat(MAX_CLIPBOARD_BYTES);

        assert_eq!(inspect_payload(&maximum, None), PayloadDecision::Write);
    }

    #[test]
    fn whitespace_only_payload_is_accepted() {
        assert_eq!(inspect_payload(" \t\r\n ", None), PayloadDecision::Write);
    }
}
