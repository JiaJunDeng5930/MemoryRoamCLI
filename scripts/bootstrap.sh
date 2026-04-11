#!/usr/bin/env bash

set -euo pipefail

PRE_COMMIT_COMMAND=""

log() {
  printf '==> %s\n' "$1"
}

fail() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}

warn() {
  printf 'warning: %s\n' "$1" >&2
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

ensure_python_venv() {
  local probe_dir

  require_command mktemp
  probe_dir="$(mktemp -d)"
  if ! python3 -m venv "$probe_dir/venv" >/dev/null 2>&1; then
    rm -rf "$probe_dir"
    fail "python3 venv support is required"
  fi

  rm -rf "$probe_dir"
}

user_bin_dir() {
  python3 -c 'import site; print(site.getuserbase())'
}

user_bin_on_path() {
  local user_bin

  user_bin="$(user_bin_dir)/bin"
  case ":$PATH:" in
    *":$user_bin:"*) return 0 ;;
    *) return 1 ;;
  esac
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
  log "Installing Rust toolchain and required components"
  rustup toolchain install stable --profile minimal --component clippy --component rustfmt
}

install_pre_commit() {
  local existing_pre_commit
  local shared_install_root
  local shared_virtualenv_dir
  local user_bin
  local wrapper_path

  existing_pre_commit="$(command -v pre-commit || true)"
  if [[ -n "$existing_pre_commit" ]] \
    && "$existing_pre_commit" --version >/dev/null 2>&1 \
    && [[ -z "${VIRTUAL_ENV:-}" || "$existing_pre_commit" != "$VIRTUAL_ENV/"* ]]; then
    PRE_COMMIT_COMMAND="$existing_pre_commit"
    log "Using existing pre-commit at $PRE_COMMIT_COMMAND"
    return
  fi

  ensure_python_venv
  shared_install_root="${XDG_DATA_HOME:-$HOME/.local/share}/memoryroam-cli"
  shared_virtualenv_dir="$shared_install_root/pre-commit-venv"
  user_bin="$(user_bin_dir)/bin"
  wrapper_path="$user_bin/pre-commit"

  log "Installing pre-commit into $shared_virtualenv_dir"
  mkdir -p "$shared_install_root"
  mkdir -p "$user_bin"
  python3 -m venv "$shared_virtualenv_dir"
  "$shared_virtualenv_dir/bin/python" -m pip install pre-commit
  printf '%s\n' \
    '#!/usr/bin/env bash' \
    "exec \"$shared_virtualenv_dir/bin/pre-commit\" \"\$@\"" \
    > "$wrapper_path"
  chmod +x "$wrapper_path"
  PRE_COMMIT_COMMAND="$shared_virtualenv_dir/bin/pre-commit"

  if ! user_bin_on_path; then
    warn "pre-commit was installed to $wrapper_path, but $user_bin is not on PATH in this shell"
  fi
}

install_git_hooks() {
  log "Installing repository hooks"
  "$PRE_COMMIT_COMMAND" install --install-hooks
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

  ensure_git_repository_root
  load_cargo_environment
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
