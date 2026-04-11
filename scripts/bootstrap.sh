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

ensure_non_root_user() {
  if [[ "${EUID}" -eq 0 ]]; then
    fail "run this script as a normal user, not as root"
  fi
}

ensure_repository_root() {
  local repository_root

  if ! repository_root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
    fail "run this script from inside the repository worktree"
  fi

  cd "$repository_root"
}

ensure_ubuntu_2404() {
  if [[ ! -r /etc/os-release ]]; then
    fail "unable to read /etc/os-release"
  fi

  # shellcheck disable=SC1091
  . /etc/os-release

  if [[ "${ID:-}" != "ubuntu" ]]; then
    fail "this script only supports Ubuntu 24.04"
  fi

  if [[ "${VERSION_ID:-}" != "24.04" ]]; then
    fail "this script only supports Ubuntu 24.04"
  fi
}

apt_get() {
  require_command sudo
  sudo env DEBIAN_FRONTEND=noninteractive apt-get "$@"
}

install_system_packages() {
  log "Installing Ubuntu development packages"
  apt_get update
  apt_get install -y \
    build-essential \
    ca-certificates \
    curl \
    git \
    pkg-config \
    pre-commit \
    python3 \
    python3-pip \
    python3-venv
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

  if [[ -x "$HOME/.cargo/bin/rustup" ]]; then
    export PATH="$HOME/.cargo/bin:$PATH"
    return
  fi

  log "Installing rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
}

install_rust_toolchain() {
  log "Installing Rust toolchain"
  rustup toolchain install stable --profile minimal --component clippy --component rustfmt
}

install_git_hooks() {
  log "Installing pre-commit hooks"
  pre-commit install --install-hooks
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
  ensure_non_root_user

  ensure_repository_root
  ensure_ubuntu_2404
  install_system_packages

  load_cargo_environment
  install_rustup_if_missing
  load_cargo_environment

  require_command rustup
  require_command cargo
  require_command pre-commit

  install_rust_toolchain
  install_git_hooks
  configure_repository_git_settings
  prefetch_cargo_dependencies

  log "Bootstrap complete"
}

main "$@"
