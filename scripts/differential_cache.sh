#!/usr/bin/env bash
# Differential cache harness wrapper.
#
# CI default (bounded): every fixtures/ entry × fixed seeds × small N.
# Local longer: ASYNC_DOCTOR_DIFF_EDITS=12 ASYNC_DOCTOR_DIFF_SEEDS=7,11,13 ./scripts/differential_cache.sh --extended
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

EXTENDED=0
FILTER=differential_ci_seed_suite
EXTRA_ARGS=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --extended)
      EXTENDED=1
      FILTER=differential_local_extended
      EXTRA_ARGS+=(--ignored)
      shift
      ;;
    --multipkg)
      FILTER=differential_m2_multipkg_other_py
      shift
      ;;
    *)
      EXTRA_ARGS+=("$1")
      shift
      ;;
  esac
done

if [[ "$EXTENDED" -eq 1 ]]; then
  export ASYNC_DOCTOR_DIFF_EDITS="${ASYNC_DOCTOR_DIFF_EDITS:-8}"
  export ASYNC_DOCTOR_DIFF_SEEDS="${ASYNC_DOCTOR_DIFF_SEEDS:-7,11,13,17}"
else
  export ASYNC_DOCTOR_DIFF_EDITS="${ASYNC_DOCTOR_DIFF_EDITS:-2}"
  export ASYNC_DOCTOR_DIFF_SEEDS="${ASYNC_DOCTOR_DIFF_SEEDS:-1,42}"
fi

echo "differential_cache: edits=$ASYNC_DOCTOR_DIFF_EDITS seeds=$ASYNC_DOCTOR_DIFF_SEEDS filter=$FILTER"
exec cargo test -p async_doctor_ty "${FILTER}" -- --nocapture "${EXTRA_ARGS[@]}"
