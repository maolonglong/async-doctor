#!/usr/bin/env bash
# Reproducible public benchmark for async-doctor.
#
# Clones pinned commits from bench/repos.toml, creates uv venvs, runs
# async-doctor cold + warm with an isolated cache, and prints a markdown table.
#
# Usage (from repo root):
#   ./bench/run.sh
#   ASYNC_DOCTOR_BIN=./target/release/async-doctor ./bench/run.sh
#   BENCH_ONLY=httpx,fastapi ./bench/run.sh
#
# Clones land under bench/.repos/ (gitignored). Nothing under that tree is
# committed.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPOS_TOML="${ROOT}/bench/repos.toml"
HARNESS="${ROOT}/bench/harness.py"
CLONE_ROOT="${ROOT}/bench/.repos"
RESULTS_JSON="${CLONE_ROOT}/results.json"
CACHE_PARENT="${CLONE_ROOT}/.cache"

ASYNC_DOCTOR_BIN="${ASYNC_DOCTOR_BIN:-${ROOT}/target/release/async-doctor}"
PYTHON_BIN="${PYTHON_BIN:-python3}"
BENCH_ONLY="${BENCH_ONLY:-}"

if [[ ! -x "${ASYNC_DOCTOR_BIN}" ]]; then
  echo "error: missing release binary at ${ASYNC_DOCTOR_BIN}" >&2
  echo "hint: cargo build -p async_doctor --release" >&2
  exit 2
fi

for req in uv git; do
  if ! command -v "${req}" >/dev/null 2>&1; then
    echo "error: ${req} is required" >&2
    exit 2
  fi
done

mkdir -p "${CLONE_ROOT}"

mapfile -d '' REPO_BLOBS < <("${PYTHON_BIN}" "${HARNESS}" parse "${REPOS_TOML}")

clone_repo() {
  local name="$1" url="$2" commit="$3"
  local dest="${CLONE_ROOT}/${name}"
  if [[ -d "${dest}/.git" ]]; then
    git -C "${dest}" fetch --depth 1 origin "${commit}" >/dev/null 2>&1 || true
    git -C "${dest}" checkout --force --detach "${commit}" >/dev/null
    git -C "${dest}" reset --hard "${commit}" >/dev/null
    git -C "${dest}" clean -fdx -e .venv >/dev/null
  else
    rm -rf "${dest}"
    git clone --filter=blob:none --no-checkout "${url}" "${dest}"
    git -C "${dest}" fetch --depth 1 origin "${commit}"
    git -C "${dest}" checkout --force --detach "${commit}"
  fi
  local got
  got="$(git -C "${dest}" rev-parse HEAD)"
  if [[ "${got}" != "${commit}" ]]; then
    echo "error: ${name} HEAD ${got} != pinned ${commit}" >&2
    exit 2
  fi
}

ensure_venv() {
  local dest="$1"
  shift
  local -a install_cmd=("$@")
  local stamp="${dest}/.venv/.async-doctor-bench-installed"
  if [[ ! -d "${dest}/.venv" ]]; then
    uv venv "${dest}/.venv"
  fi
  if [[ -f "${stamp}" && "${BENCH_FORCE_INSTALL:-}" != "1" ]]; then
    return 0
  fi
  if ((${#install_cmd[@]})); then
    set +e
    (
      cd "${dest}"
      # shellcheck disable=SC1091
      source "${dest}/.venv/bin/activate"
      uv "${install_cmd[@]}"
    )
    local rc=$?
    set -e
    if [[ ${rc} -ne 0 ]]; then
      echo "warning: uv install failed for $(basename "${dest}") (rc=${rc}); continuing with partial/empty site-packages" >&2
    else
      : >"${stamp}"
    fi
  else
    : >"${stamp}"
  fi
}

RESULTS=()

for blob in "${REPO_BLOBS[@]}"; do
  [[ -z "${blob}" ]] && continue
  IFS='|' read -r name url commit scan install_joined <<<"${blob}"
  IFS=$'\x1f' read -r -a install_cmd <<<"${install_joined}"

  if [[ -n "${BENCH_ONLY}" ]]; then
    case ",${BENCH_ONLY}," in
      *",${name},"*) ;;
      *) continue ;;
    esac
  fi

  echo "==> ${name} @ ${commit:0:12}" >&2
  clone_repo "${name}" "${url}" "${commit}"
  ensure_venv "${CLONE_ROOT}/${name}" "${install_cmd[@]}"

  scan_path="${CLONE_ROOT}/${name}/${scan}"
  if [[ ! -d "${scan_path}" ]]; then
    echo "error: scan path missing: ${scan_path}" >&2
    exit 2
  fi

  files="$("${PYTHON_BIN}" "${HARNESS}" count "${scan_path}")"

  repo_cache="${CACHE_PARENT}/${name}"
  rm -rf "${repo_cache}"
  mkdir -p "${repo_cache}"

  cold_json="${CLONE_ROOT}/${name}.cold.json"
  warm_json="${CLONE_ROOT}/${name}.warm.json"
  venv_dir="${CLONE_ROOT}/${name}/.venv"

  cold_line="$(
    XDG_CACHE_HOME="${repo_cache}" \
      "${PYTHON_BIN}" "${HARNESS}" run \
      "${ASYNC_DOCTOR_BIN}" "${scan_path}" "${cold_json}" "${venv_dir}"
  )"
  warm_line="$(
    XDG_CACHE_HOME="${repo_cache}" \
      "${PYTHON_BIN}" "${HARNESS}" run \
      "${ASYNC_DOCTOR_BIN}" "${scan_path}" "${warm_json}" "${venv_dir}"
  )"

  IFS='|' read -r cold_s cold_rss cold_findings cold_cands <<<"${cold_line}"
  IFS='|' read -r warm_s warm_rss warm_findings warm_cands <<<"${warm_line}"

  findings="${cold_findings}"
  candidates="${cold_cands}"
  peak_rss_kib="${cold_rss}"
  if [[ "${warm_rss}" -gt "${cold_rss}" ]]; then
    peak_rss_kib="${warm_rss}"
  fi

  if cmp -s "${cold_json}" "${warm_json}"; then
    identical="yes"
  else
    identical="no"
  fi

  RESULTS+=("${name}|${commit}|${scan}|${files}|${findings}|${candidates}|${cold_s}|${warm_s}|${peak_rss_kib}|${identical}")
  echo "    files=${files} findings=${findings} candidates=${candidates} cold=${cold_s}s warm=${warm_s}s rss=${peak_rss_kib}KiB identical=${identical}" >&2
done

RESULTS_TSV="${CLONE_ROOT}/results.tsv"
printf '%s\n' "${RESULTS[@]}" >"${RESULTS_TSV}"
"${PYTHON_BIN}" "${HARNESS}" emit "${RESULTS_JSON}" "${RESULTS_TSV}"
