#!/usr/bin/env bash

set -euo pipefail

log() {
  printf '==> %s\n' "$1"
}

fail() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}

require_command() {
  local command_name="$1"

  if ! command -v "$command_name" >/dev/null 2>&1; then
    fail "required command not found: $command_name"
  fi
}

ensure_git_repository_root() {
  local repository_root

  if ! repository_root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
    fail "run this script from inside the repository worktree"
  fi

  cd "$repository_root"
}

ensure_python_pip() {
  if ! python3 -m pip --version >/dev/null 2>&1; then
    fail "python3 -m pip is required"
  fi
}

load_cargo_environment() {
  if [[ -f "$HOME/.cargo/env" ]]; then
    # shellcheck disable=SC1090
    . "$HOME/.cargo/env"
  fi
}

install_rustup_if_missing() {
  if command -v rustup >/dev/null 2>&1; then
    return
  fi

  log "Installing rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
}

install_rust_toolchain() {
  log "Installing Rust toolchain and required components"
  rustup toolchain install stable --profile minimal --component clippy --component rustfmt
  rustup default stable
}

install_pre_commit() {
  log "Installing pre-commit"
  python3 -m pip install --user pre-commit
}

install_git_hooks() {
  log "Installing repository hooks"
  python3 -m pre_commit install --install-hooks
}

configure_repository_git_settings() {
  log "Configuring repository-local Git settings"
  git config --local pull.rebase true
  git config --local branch.autoSetupRebase always
  git config --local merge.ff only
}

prefetch_cargo_dependencies() {
  log "Fetching workspace Cargo dependencies"
  cargo fetch --locked
}

main() {
  require_command git
  require_command curl
  require_command python3
  ensure_python_pip

  ensure_git_repository_root
  install_rustup_if_missing
  load_cargo_environment
  require_command rustup
  require_command cargo

  install_rust_toolchain
  install_pre_commit
  install_git_hooks
  configure_repository_git_settings
  prefetch_cargo_dependencies

  log "Bootstrap complete"
}

main "$@"
