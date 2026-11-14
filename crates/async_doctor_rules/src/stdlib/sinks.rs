//! Stdlib sink rows assembled from domain submodules.

mod filesystem;
mod network;
mod runtime;

use crate::BuiltinSink;

use filesystem::FILESYSTEM_SINKS;
use network::NETWORK_SINKS;
use runtime::RUNTIME_SINKS;

const TOTAL_STDLIB_SINKS: usize =
    NETWORK_SINKS.len() + FILESYSTEM_SINKS.len() + RUNTIME_SINKS.len();

const fn concat_stdlib_sinks() -> [BuiltinSink; TOTAL_STDLIB_SINKS] {
    let mut out = [NETWORK_SINKS[0]; TOTAL_STDLIB_SINKS];
    let mut idx = 0;

    let mut i = 0;
    while i < NETWORK_SINKS.len() {
        out[idx] = NETWORK_SINKS[i];
        idx += 1;
        i += 1;
    }

    let mut i = 0;
    while i < FILESYSTEM_SINKS.len() {
        out[idx] = FILESYSTEM_SINKS[i];
        idx += 1;
        i += 1;
    }

    let mut i = 0;
    while i < RUNTIME_SINKS.len() {
        out[idx] = RUNTIME_SINKS[i];
        idx += 1;
        i += 1;
    }

    out
}

static STDLIB_SINKS_ARRAY: [BuiltinSink; TOTAL_STDLIB_SINKS] = concat_stdlib_sinks();

/// Stdlib sink catalog.
pub static STDLIB_SINKS: &[BuiltinSink] = &STDLIB_SINKS_ARRAY;
