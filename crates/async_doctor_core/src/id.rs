//! Stable finding ids (baseline-friendly; independent of line/column).

use sha2::{Digest, Sha256};

/// Inputs that uniquely identify a finding for baseline / dedupe.
///
/// M1 rule: hash `rule` + entry qualified name + sink qualified name + chain
/// qualified names (+ occurrence ordinal). **Not** file/line/column.
#[derive(Debug, Clone, Copy)]
pub struct FindingIdParts<'a> {
    pub rule: &'a str,
    /// Qualified entry name, e.g. `pkg.mod:handler` or `pkg.mod:A.handle`.
    pub entry_qualname: &'a str,
    /// Sink qualified name, e.g. `time.sleep`.
    pub sink_qualname: &'a str,
    /// Intermediate + sink hop qualified names along the evidence chain
    /// (callee names for each hop, excluding the entry itself).
    pub chain_qualnames: &'a [&'a str],
    /// Distinguishes multiple identical chains from the same entry (e.g. two
    /// direct `time.sleep` calls in one function).
    pub occurrence: u32,
}

/// Stable finding id: sha256 truncated to 16 bytes hex.
pub fn finding_id(parts: FindingIdParts<'_>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(parts.rule.as_bytes());
    hasher.update(b"\0");
    hasher.update(parts.entry_qualname.as_bytes());
    hasher.update(b"\0");
    hasher.update(parts.sink_qualname.as_bytes());
    hasher.update(b"\0");
    for (i, name) in parts.chain_qualnames.iter().enumerate() {
        if i > 0 {
            hasher.update(b"\0");
        }
        hasher.update(name.as_bytes());
    }
    hasher.update(b"\0");
    hasher.update(parts.occurrence.to_string().as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..16])
}

pub fn fix_group_id(rule: &str, sink: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(rule.as_bytes());
    hasher.update(b"\0");
    hasher.update(sink.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..12])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finding_id_is_stable() {
        let chain = ["time.sleep"];
        let a = finding_id(FindingIdParts {
            rule: "blocking-sleep",
            entry_qualname: "mod:handler",
            sink_qualname: "time.sleep",
            chain_qualnames: &chain,
            occurrence: 0,
        });
        let b = finding_id(FindingIdParts {
            rule: "blocking-sleep",
            entry_qualname: "mod:handler",
            sink_qualname: "time.sleep",
            chain_qualnames: &chain,
            occurrence: 0,
        });
        assert_eq!(a, b);
    }

    #[test]
    fn finding_id_ignores_line_column_inputs() {
        // Stability across moves is ensured by not hashing locations at all.
        let chain = ["helper", "time.sleep"];
        let id = finding_id(FindingIdParts {
            rule: "blocking-sleep",
            entry_qualname: "app:run",
            sink_qualname: "time.sleep",
            chain_qualnames: &chain,
            occurrence: 0,
        });
        assert_eq!(id.len(), 32);
    }

    #[test]
    fn finding_id_distinguishes_entry_sink_chain_occurrence() {
        let chain_a = ["time.sleep"];
        let chain_b = ["helper", "time.sleep"];
        let a = finding_id(FindingIdParts {
            rule: "blocking-sleep",
            entry_qualname: "A.handle",
            sink_qualname: "time.sleep",
            chain_qualnames: &chain_a,
            occurrence: 0,
        });
        let b = finding_id(FindingIdParts {
            rule: "blocking-sleep",
            entry_qualname: "B.handle",
            sink_qualname: "time.sleep",
            chain_qualnames: &chain_a,
            occurrence: 0,
        });
        assert_ne!(a, b);

        let multi_hop = finding_id(FindingIdParts {
            rule: "blocking-sleep",
            entry_qualname: "A.handle",
            sink_qualname: "time.sleep",
            chain_qualnames: &chain_b,
            occurrence: 0,
        });
        assert_ne!(a, multi_hop);

        let second = finding_id(FindingIdParts {
            rule: "blocking-sleep",
            entry_qualname: "A.handle",
            sink_qualname: "time.sleep",
            chain_qualnames: &chain_a,
            occurrence: 1,
        });
        assert_ne!(a, second);
    }
}
