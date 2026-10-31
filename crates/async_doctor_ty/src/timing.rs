//! Optional phase timing for profiling (`ASYNC_DOCTOR_TIMING=1`).

use std::time::{Duration, Instant};

/// When `ASYNC_DOCTOR_TIMING` is set and non-empty, record named spans and
/// print a one-line summary to stderr at drop (or [`Self::finish`]).
pub(crate) struct PhaseTiming {
    enabled: bool,
    start: Instant,
    spans: Vec<(&'static str, Duration)>,
}

impl PhaseTiming {
    pub(crate) fn from_env() -> Self {
        let enabled = std::env::var_os("ASYNC_DOCTOR_TIMING").is_some_and(|v| !v.is_empty());
        Self {
            enabled,
            start: Instant::now(),
            spans: Vec::new(),
        }
    }

    pub(crate) fn span(&mut self, name: &'static str, started: Instant) {
        if self.enabled {
            self.spans.push((name, started.elapsed()));
        }
    }

    pub(crate) fn finish(mut self, note: &str) {
        if !self.enabled {
            return;
        }
        let total = self.start.elapsed();
        let mut parts = Vec::with_capacity(self.spans.len() + 2);
        for (name, d) in &self.spans {
            parts.push(format!("{name}={:.3}s", d.as_secs_f64()));
        }
        if !note.is_empty() {
            parts.push(note.to_string());
        }
        parts.push(format!("total={:.3}s", total.as_secs_f64()));
        eprintln!("async-doctor timing: {}", parts.join(" "));
        // Prevent Drop from double-printing.
        self.enabled = false;
        self.spans.clear();
    }
}

impl Drop for PhaseTiming {
    fn drop(&mut self) {
        if !self.enabled || self.spans.is_empty() {
            return;
        }
        // finish() was not called — still emit what we have.
        let total = self.start.elapsed();
        let parts: Vec<String> = self
            .spans
            .iter()
            .map(|(n, d)| format!("{n}={:.3}s", d.as_secs_f64()))
            .chain(std::iter::once(format!(
                "total={:.3}s",
                total.as_secs_f64()
            )))
            .collect();
        eprintln!("async-doctor timing: {}", parts.join(" "));
        self.spans.clear();
    }
}
