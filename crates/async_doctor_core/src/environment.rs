//! Scan-time Python environment summary for reports and text UX.

use serde::{Deserialize, Serialize};

/// Kind of interpreter/venv used for resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentKind {
    /// Activated or in-project virtualenv (pyvenv.cfg / VIRTUAL_ENV / conda env).
    Venv,
    /// System / base interpreter (no project venv).
    System,
    /// Nothing resolvable.
    None,
}

impl EnvironmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Venv => "venv",
            Self::System => "system",
            Self::None => "none",
        }
    }
}

/// Which interpreter / venv / site-packages the scan used.
///
/// Always present on [`crate::Report`] so agents can see silent degradation when
/// third-party packages are missing because no project venv was active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanEnvironment {
    pub kind: EnvironmentKind,
    /// Discovery origin label (`VIRTUAL_ENV`, `local .venv`, `system`, `none`, …).
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<String>,
    /// site-packages directories on the module search path.
    pub site_packages: Vec<String>,
    /// Detected async frameworks and key libraries (`<name> <version>`) in `site_packages`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frameworks: Vec<String>,
    /// Distinct top-level third-party modules that failed to import.
    pub unresolved_third_party: u32,
    /// Distinct top-level third-party modules that resolved successfully.
    pub resolved_third_party: u32,
    /// Up to [`Self::MAX_UNRESOLVED_THIRD_PARTY_MODULES`] sorted top-level
    /// third-party module names that failed to import (omitted when empty).
    #[serde(
        rename = "unresolvedThirdPartyModules",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub unresolved_third_party_modules: Vec<String>,
    /// Short advisory when resolution may be degraded (empty when fine).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Third-party callees skipped because the follow budget was reached.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub follow_budget_skipped: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

impl ScanEnvironment {
    /// Warn when unresolved third-party imports are at least this fraction of
    /// (unresolved + resolved) third-party heads, while a venv is active.
    pub const UNRESOLVED_RATIO_THRESHOLD: f64 = 0.25;

    /// Maximum number of unresolved third-party module names surfaced in reports.
    pub const MAX_UNRESOLVED_THIRD_PARTY_MODULES: usize = 10;

    pub fn none() -> Self {
        Self {
            kind: EnvironmentKind::None,
            origin: "none".into(),
            prefix: None,
            interpreter: None,
            site_packages: Vec::new(),
            frameworks: Vec::new(),
            unresolved_third_party: 0,
            resolved_third_party: 0,
            unresolved_third_party_modules: Vec::new(),
            note: String::new(),
            follow_budget_skipped: 0,
        }
    }

    /// Refresh [`Self::note`] from kind + third-party resolve counts.
    ///
    /// - No project venv + zero unresolved → silent (stdlib-only / nothing missing).
    /// - No project venv + some unresolved → activate / `VIRTUAL_ENV` note.
    /// - Project venv + high unresolved ratio → install deps/extras note.
    /// - Project venv + low ratio → silent (optional deps are expected).
    pub fn refresh_note(&mut self) {
        self.note = match self.kind {
            EnvironmentKind::None | EnvironmentKind::System => {
                if self.unresolved_third_party == 0 {
                    String::new()
                } else {
                    "activate the project venv or set VIRTUAL_ENV so third-party imports resolve"
                        .to_string()
                }
            }
            EnvironmentKind::Venv => {
                let unresolved = self.unresolved_third_party;
                let total = unresolved.saturating_add(self.resolved_third_party);
                if unresolved == 0 || total == 0 {
                    String::new()
                } else {
                    let ratio = f64::from(unresolved) / f64::from(total);
                    if ratio >= Self::UNRESOLVED_RATIO_THRESHOLD {
                        format!(
                            "{unresolved} third-party modules are not installed in this venv; \
                             install the project's deps/extras for full coverage"
                        )
                    } else {
                        String::new()
                    }
                }
            }
        };
    }

    pub fn with_third_party_counts(mut self, unresolved: u32, resolved: u32) -> Self {
        self.unresolved_third_party = unresolved;
        self.resolved_third_party = resolved;
        self.refresh_note();
        self
    }

    /// Reason string for the location-less missing-third-party aggregate.
    ///
    /// Wording follows whether a project venv is already active — do not tell
    /// the user to "run in the project venv" when they already are.
    pub fn missing_third_party_reason(n: u32, kind: EnvironmentKind) -> String {
        match kind {
            EnvironmentKind::Venv => format!(
                "{n} third-party modules not installed in the active venv; \
                 install the project's deps/extras"
            ),
            EnvironmentKind::System | EnvironmentKind::None => {
                format!("{n} third-party modules not installed; run in the project venv")
            }
        }
    }
}

impl Default for ScanEnvironment {
    fn default() -> Self {
        Self::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_when_no_venv_and_zero_unresolved() {
        let mut env = ScanEnvironment::none();
        env.refresh_note();
        assert!(
            env.note.is_empty(),
            "stdlib-only must stay quiet: {:?}",
            env.note
        );
    }

    #[test]
    fn note_when_no_venv_and_unresolved() {
        let mut env = ScanEnvironment::none();
        env.unresolved_third_party = 2;
        env.refresh_note();
        assert!(env.note.contains("VIRTUAL_ENV"), "{:?}", env.note);
    }

    #[test]
    fn venv_high_ratio_asks_for_deps_not_activate() {
        let mut env = ScanEnvironment {
            kind: EnvironmentKind::Venv,
            origin: "VIRTUAL_ENV".into(),
            prefix: Some("/proj/.venv".into()),
            interpreter: Some("/proj/.venv/bin/python".into()),
            site_packages: vec!["/proj/.venv/lib/python3.12/site-packages".into()],
            frameworks: Vec::new(),
            unresolved_third_party: 98,
            resolved_third_party: 50,
            unresolved_third_party_modules: Vec::new(),
            note: String::new(),
            follow_budget_skipped: 0,
        };
        env.refresh_note();
        assert!(
            env.note.contains("not installed in this venv"),
            "expected deps note, got {:?}",
            env.note
        );
        assert!(
            !env.note.contains("VIRTUAL_ENV"),
            "must not ask to set VIRTUAL_ENV when already in a venv: {:?}",
            env.note
        );
    }

    #[test]
    fn venv_low_ratio_stays_quiet() {
        let mut env = ScanEnvironment {
            kind: EnvironmentKind::Venv,
            origin: "VIRTUAL_ENV".into(),
            prefix: Some("/proj/.venv".into()),
            interpreter: Some("/proj/.venv/bin/python".into()),
            site_packages: vec!["/proj/.venv/lib/python3.12/site-packages".into()],
            frameworks: Vec::new(),
            unresolved_third_party: 2,
            resolved_third_party: 40,
            unresolved_third_party_modules: Vec::new(),
            note: String::new(),
            follow_budget_skipped: 0,
        };
        env.refresh_note();
        assert!(
            env.note.is_empty(),
            "low ratio must stay quiet: {:?}",
            env.note
        );
    }

    #[test]
    fn location_less_reason_follows_venv_kind() {
        let venv = ScanEnvironment::missing_third_party_reason(17, EnvironmentKind::Venv);
        assert!(
            venv.contains("active venv") && venv.contains("deps/extras"),
            "{venv}"
        );
        assert!(!venv.contains("run in the project venv"), "{venv}");

        let system = ScanEnvironment::missing_third_party_reason(3, EnvironmentKind::System);
        assert!(system.contains("run in the project venv"), "{system}");
        assert!(!system.contains("active venv"), "{system}");

        let none = ScanEnvironment::missing_third_party_reason(1, EnvironmentKind::None);
        assert!(none.contains("run in the project venv"), "{none}");
    }
}
