//! Stuck detection — screen hash tracking, repetition detection, drift alerts.
//!
//! Monitors screen state and action history to detect when the agent may be
//! stuck in a loop, repeatedly performing the same action, or navigating
//! aimlessly without progress.

use std::collections::VecDeque;

use super::types::{StuckAlert, UIElement};

/// How many screen hashes to keep for unchanged detection.
const SCREEN_HISTORY_SIZE: usize = 8;

/// Threshold for consecutive unchanged screens.
const UNCHANGED_THRESHOLD: usize = 3;

/// How many actions to keep in the sliding window.
const ACTION_HISTORY_SIZE: usize = 8;

/// Threshold for same-action repetition in the window.
const REPEAT_THRESHOLD: usize = 3;

/// Navigation actions that indicate potential drift.
const NAV_ACTIONS: &[&str] = &["back", "home", "recent"];

/// Threshold for navigation drift in the action window.
const DRIFT_THRESHOLD: usize = 4;

/// Navigation drift window (how many recent actions to check).
const DRIFT_WINDOW: usize = 5;

/// Detects when the agent may be stuck.
#[derive(Debug)]
pub struct StuckDetector {
    /// Recent screen hashes for unchanged detection.
    screen_hashes: VecDeque<String>,
    /// Recent action signatures for repetition/drift detection.
    action_history: VecDeque<String>,
}

impl Default for StuckDetector {
    fn default() -> Self {
        Self {
            screen_hashes: VecDeque::with_capacity(SCREEN_HISTORY_SIZE),
            action_history: VecDeque::with_capacity(ACTION_HISTORY_SIZE),
        }
    }
}

impl StuckDetector {
    /// Compute a hash of the current screen state from elements.
    pub fn hash_screen(elements: &[UIElement]) -> String {
        let mut parts: Vec<String> = elements.iter().map(|e| e.hash_key()).collect();
        parts.sort();
        parts.join(";")
    }

    /// Record a screen observation and return any alerts.
    pub fn observe_screen(&mut self, elements: &[UIElement]) -> Vec<StuckAlert> {
        let hash = Self::hash_screen(elements);
        let mut alerts = Vec::new();

        // Check for unchanged screen
        let consecutive_same = self
            .screen_hashes
            .iter()
            .rev()
            .take_while(|h| **h == hash)
            .count();

        if consecutive_same >= UNCHANGED_THRESHOLD - 1 {
            // Current observation makes it UNCHANGED_THRESHOLD
            alerts.push(StuckAlert::ScreenUnchanged(format!(
                "Screen unchanged for {} consecutive observations. \
                 Try a different action or scroll to reveal new elements.",
                consecutive_same + 1
            )));
        }

        // Add to history
        if self.screen_hashes.len() >= SCREEN_HISTORY_SIZE {
            self.screen_hashes.pop_front();
        }
        self.screen_hashes.push_back(hash);

        alerts
    }

    /// Record an action and return any alerts.
    pub fn observe_action(&mut self, action: &str) -> Vec<StuckAlert> {
        let mut alerts = Vec::new();
        let sig = action.to_lowercase();

        // Check for repeated action
        let repeat_count = self
            .action_history
            .iter()
            .rev()
            .take(ACTION_HISTORY_SIZE)
            .filter(|a| **a == sig)
            .count();

        if repeat_count >= REPEAT_THRESHOLD - 1 {
            alerts.push(StuckAlert::ActionRepeated(format!(
                "Action '{}' repeated {} times in the last {} actions. \
                 Consider a different approach.",
                action,
                repeat_count + 1,
                ACTION_HISTORY_SIZE
            )));
        }

        // Check for navigation drift
        let recent: Vec<&String> = self
            .action_history
            .iter()
            .rev()
            .take(DRIFT_WINDOW - 1)
            .collect();
        let nav_count = recent
            .iter()
            .filter(|a| NAV_ACTIONS.contains(&a.as_str()))
            .count()
            + if NAV_ACTIONS.contains(&sig.as_str()) {
                1
            } else {
                0
            };

        if nav_count >= DRIFT_THRESHOLD {
            alerts.push(StuckAlert::NavigationDrift(format!(
                "{} navigation actions (back/home/recent) in the last {} actions. \
                 The agent may be navigating without clear purpose.",
                nav_count, DRIFT_WINDOW
            )));
        }

        // Add to history
        if self.action_history.len() >= ACTION_HISTORY_SIZE {
            self.action_history.pop_front();
        }
        self.action_history.push_back(sig);

        alerts
    }

    /// Clear all history (e.g., when starting a new task).
    pub fn reset(&mut self) {
        self.screen_hashes.clear();
        self.action_history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_elements(text: &str) -> Vec<UIElement> {
        vec![UIElement {
            text: text.into(),
            center: [100, 200],
            action: "tap".into(),
            class: None,
            id: Some("btn".into()),
            hint: None,
            enabled: true,
            checked: false,
            focused: false,
            editable: false,
            scrollable: false,
            score: 10,
        }]
    }

    #[test]
    fn test_hash_screen_deterministic() {
        let elems = make_elements("OK");
        let h1 = StuckDetector::hash_screen(&elems);
        let h2 = StuckDetector::hash_screen(&elems);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_screen_differs() {
        let h1 = StuckDetector::hash_screen(&make_elements("OK"));
        let h2 = StuckDetector::hash_screen(&make_elements("Cancel"));
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_screen_unchanged_alert() {
        let mut detector = StuckDetector::default();
        let elems = make_elements("Same");

        // First observation: no alert
        let alerts = detector.observe_screen(&elems);
        assert!(alerts.is_empty());

        // Second: no alert
        let alerts = detector.observe_screen(&elems);
        assert!(alerts.is_empty());

        // Third: alert! (3 consecutive)
        let alerts = detector.observe_screen(&elems);
        assert_eq!(alerts.len(), 1);
        assert!(matches!(alerts[0], StuckAlert::ScreenUnchanged(_)));
    }

    #[test]
    fn test_screen_changed_resets_count() {
        let mut detector = StuckDetector::default();

        detector.observe_screen(&make_elements("A"));
        detector.observe_screen(&make_elements("A"));
        // Change!
        detector.observe_screen(&make_elements("B"));
        // Back to A, but count resets
        let alerts = detector.observe_screen(&make_elements("A"));
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_action_repeated_alert() {
        let mut detector = StuckDetector::default();

        detector.observe_action("tap");
        detector.observe_action("tap");
        let alerts = detector.observe_action("tap");
        assert_eq!(alerts.len(), 1);
        assert!(matches!(alerts[0], StuckAlert::ActionRepeated(_)));
    }

    #[test]
    fn test_action_mixed_no_alert() {
        let mut detector = StuckDetector::default();

        let alerts1 = detector.observe_action("tap");
        let alerts2 = detector.observe_action("type");
        let alerts3 = detector.observe_action("scroll");
        assert!(alerts1.is_empty());
        assert!(alerts2.is_empty());
        assert!(alerts3.is_empty());
    }

    #[test]
    fn test_navigation_drift_alert() {
        let mut detector = StuckDetector::default();

        detector.observe_action("back");
        detector.observe_action("home");
        detector.observe_action("back");
        let alerts = detector.observe_action("back");
        assert!(alerts
            .iter()
            .any(|a| matches!(a, StuckAlert::NavigationDrift(_))));
    }

    #[test]
    fn test_reset_clears_history() {
        let mut detector = StuckDetector::default();

        detector.observe_screen(&make_elements("Same"));
        detector.observe_screen(&make_elements("Same"));
        detector.observe_action("tap");
        detector.observe_action("tap");

        detector.reset();

        // After reset, no alerts for same observations
        let alerts = detector.observe_screen(&make_elements("Same"));
        assert!(alerts.is_empty());
        let alerts = detector.observe_action("tap");
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_history_capacity() {
        let mut detector = StuckDetector::default();

        // Fill beyond capacity
        for i in 0..20 {
            detector.observe_screen(&make_elements(&format!("elem_{}", i)));
            detector.observe_action(&format!("action_{}", i));
        }

        // Should not panic, history is bounded
        assert!(detector.screen_hashes.len() <= SCREEN_HISTORY_SIZE);
        assert!(detector.action_history.len() <= ACTION_HISTORY_SIZE);
    }
}
