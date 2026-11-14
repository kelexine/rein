// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-14
// Purpose: ModeManager for runtime profile switching and event-log recording

use rein_core::{CoreError, EventLog, ModeId, SessionEvent};
use rein_policy::PolicyEngine;

use crate::modes::Profile;

/// Manages the active [`Profile`] and synchronizes policy rules on mode switches.
#[derive(Debug)]
pub struct ModeManager {
    active: Profile,
}

impl ModeManager {
    /// Initializes [`ModeManager`] with the given starting [`Profile`].
    #[must_use]
    pub fn new(initial: Profile) -> Self {
        Self { active: initial }
    }

    /// Returns a reference to the currently active [`Profile`].
    #[must_use]
    pub fn active(&self) -> &Profile {
        &self.active
    }

    /// Switches to `next`, updates `policy`, and appends a [`SessionEvent::ModeSwitched`] record.
    pub fn switch_to(
        &mut self,
        next: Profile,
        policy: &mut PolicyEngine,
        log: &EventLog,
    ) -> Result<ModeId, CoreError> {
        let mode = next.mode;
        policy.set_rules(next.policy_rules.clone());
        self.active = next;
        log.append(&SessionEvent::ModeSwitched { mode })?;
        Ok(mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rein_core::MemoryScope;
    use rein_policy::{AuditLog, WorkspaceSandbox};

    #[test]
    fn coding_profile_sees_personal_memory_and_switches_cleanly()
    -> Result<(), Box<dyn std::error::Error>> {
        let coding = Profile::coding();
        assert!(
            coding
                .readable_memory_scopes
                .contains(&MemoryScope::Personal)
        );

        let temp_dir = std::env::temp_dir();
        let event_path = temp_dir.join("rein_mode_switch_events.jsonl");
        let audit_path = temp_dir.join("rein_mode_switch_audit.jsonl");
        let _ = std::fs::remove_file(&event_path);
        let _ = std::fs::remove_file(&audit_path);

        let log = EventLog::new(&event_path);
        let mut policy = PolicyEngine::new(
            coding.policy_rules.clone(),
            WorkspaceSandbox::new(&temp_dir),
            AuditLog::new(&audit_path),
        );
        let mut manager = ModeManager::new(coding);
        let new_mode = manager.switch_to(Profile::personal(), &mut policy, &log)?;
        assert_eq!(new_mode, ModeId::Personal);

        let _ = std::fs::remove_file(&event_path);
        let _ = std::fs::remove_file(&audit_path);
        Ok(())
    }
}
