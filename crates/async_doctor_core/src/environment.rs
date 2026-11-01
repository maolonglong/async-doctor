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
    /// Distinct top-level third-party modules that failed to import.
    pub unresolved_third_party: u32,
    /// Short advisory when resolution may be degraded (empty when fine).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl ScanEnvironment {
    /// Threshold for "many" unresolved third-party imports that warrants a note
    /// even when a project venv was found.
    pub const MANY_UNRESOLVED_THIRD_PARTY: u32 = 3;

    pub fn none() -> Self {
        Self {
            kind: EnvironmentKind::None,
            origin: "none".into(),
            prefix: None,
            interpreter: None,
            site_packages: Vec::new(),
            unresolved_third_party: 0,
            note: String::new(),
        }
    }

    /// Refresh [`Self::note`] from kind + unresolved third-party count.
    pub fn refresh_note(&mut self) {
        let degraded = self.kind != EnvironmentKind::Venv
            || self.unresolved_third_party >= Self::MANY_UNRESOLVED_THIRD_PARTY;
        self.note = if degraded {
            "activate the project venv or set VIRTUAL_ENV so third-party imports resolve"
                .to_string()
        } else {
            String::new()
        };
    }

    pub fn with_unresolved_third_party(mut self, n: u32) -> Self {
        self.unresolved_third_party = n;
        self.refresh_note();
        self
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
    fn note_when_no_venv() {
        let mut env = ScanEnvironment::none();
        env.refresh_note();
        assert!(!env.note.is_empty());
    }

    #[test]
    fn note_when_many_unresolved_with_venv() {
        let mut env = ScanEnvironment {
            kind: EnvironmentKind::Venv,
            origin: "local .venv".into(),
            prefix: Some("/proj/.venv".into()),
            interpreter: None,
            site_packages: vec!["/proj/.venv/lib/python3.12/site-packages".into()],
            unresolved_third_party: ScanEnvironment::MANY_UNRESOLVED_THIRD_PARTY,
            note: String::new(),
        };
        env.refresh_note();
        assert!(!env.note.is_empty());
    }

    #[test]
    fn quiet_when_venv_and_few_unresolved() {
        let mut env = ScanEnvironment {
            kind: EnvironmentKind::Venv,
            origin: "VIRTUAL_ENV".into(),
            prefix: Some("/proj/.venv".into()),
            interpreter: Some("/proj/.venv/bin/python".into()),
            site_packages: vec!["/proj/.venv/lib/python3.12/site-packages".into()],
            unresolved_third_party: 1,
            note: String::new(),
        };
        env.refresh_note();
        assert!(env.note.is_empty());
    }
}
