#!/bin/sh
# Install mx-keyboard-switcher from a GitHub Release, falling back to a
# local cargo build when no matching prebuilt asset exists.
set -eu

REPO="itmaxk/mx-keyboard-switcher"
BIN="mx-keyboard-switcher"
INSTALL_DIR="${HOME}/.local/bin"

NO_AUTOSTART=0
FROM_SOURCE=0
VERSION=""

usage() {
  cat <<'EOF'
Usage: install.sh [options]

Options:
  --no-autostart   Install only; do not configure or start autostart
  --from-source    Skip prebuilt download; build with cargo
  --version TAG    Install a specific release tag (default: latest)
  --help           Show this help and exit
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --no-autostart)
      NO_AUTOSTART=1
      ;;
    --from-source)
      FROM_SOURCE=1
      ;;
    --version)
      if [ "$#" -lt 2 ]; then
        echo "error: --version requires a tag (e.g. v0.1.0)" >&2
        exit 1
      fi
      VERSION="$2"
      shift
      ;;
    --version=*)
      VERSION="${1#--version=}"
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
  shift
done

log() {
  printf '%s\n' "$*"
}

warn() {
  printf 'warning: %s\n' "$*" >&2
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

detect_triplet() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}:${arch}" in
    Linux:x86_64)
      TRIPLET="x86_64-unknown-linux-gnu"
      ;;
    Darwin:arm64|Darwin:aarch64)
      TRIPLET="aarch64-apple-darwin"
      ;;
    Darwin:x86_64)
      TRIPLET="x86_64-apple-darwin"
      ;;
    *)
      TRIPLET=""
      ;;
  esac
}

resolve_latest_tag() {
  api="https://api.github.com/repos/${REPO}/releases/latest"
  body="$(curl -fsSL "$api")" || return 1
  tag="$(printf '%s\n' "$body" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
  [ -n "$tag" ] || return 1
  printf '%s\n' "$tag"
}

download_prebuilt() {
  tag="$1"
  archive="mx-keyboard-switcher-${TRIPLET}.tar.gz"
  url="https://github.com/${REPO}/releases/download/${tag}/${archive}"
  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap 'rm -rf "$tmp"' EXIT INT HUP TERM
  log "Downloading prebuilt binary: ${url}"
  if ! curl -fL --progress-bar -o "${tmp}/${archive}" "$url"; then
    rm -rf "$tmp"
    trap - EXIT INT HUP TERM
    return 1
  fi
  if ! tar -xzf "${tmp}/${archive}" -C "$tmp"; then
    rm -rf "$tmp"
    trap - EXIT INT HUP TERM
    return 1
  fi
  if [ ! -f "${tmp}/${BIN}" ]; then
    rm -rf "$tmp"
    trap - EXIT INT HUP TERM
    return 1
  fi
  SRC_BIN="${tmp}/${BIN}"
  PREBUILT_TMP="$tmp"
}

build_from_source() {
  need_cmd cargo
  need_cmd git

  if [ -f "./crates/mxks-app/Cargo.toml" ]; then
    log "Building from local checkout..."
    cargo build --release -p mxks-app
    SRC_BIN="./target/release/${BIN}"
    return 0
  fi

  need_cmd curl
  src_tmp="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap 'rm -rf "$src_tmp" ${PREBUILT_TMP:+"$PREBUILT_TMP"}' EXIT INT HUP TERM
  log "Cloning ${REPO}..."
  git clone --depth 1 "https://github.com/${REPO}.git" "${src_tmp}/src"
  (
    cd "${src_tmp}/src"
    cargo build --release -p mxks-app
  )
  SRC_BIN="${src_tmp}/src/target/release/${BIN}"
  SOURCE_TMP="$src_tmp"
}

install_binary() {
  [ -f "$SRC_BIN" ] || die "built binary not found: ${SRC_BIN}"
  mkdir -p "$INSTALL_DIR"
  # Stop a running instance so the binary can be replaced.
  pkill -x "$BIN" 2>/dev/null || true
  install -m 755 "$SRC_BIN" "${INSTALL_DIR}/${BIN}"
  log "Installed ${INSTALL_DIR}/${BIN}"

  case ":${PATH}:" in
    *":${INSTALL_DIR}:"*)
      ;;
    *)
      log ""
      log "${INSTALL_DIR} is not in your PATH. Add this to your shell rc:"
      log "  export PATH=\"${INSTALL_DIR}:\$PATH\""
      ;;
  esac
}

write_linux_autostart() {
  desktop_dir="${HOME}/.config/autostart"
  desktop_file="${desktop_dir}/${BIN}.desktop"
  mkdir -p "$desktop_dir"
  cat >"$desktop_file" <<EOF
[Desktop Entry]
Type=Application
Name=MX Keyboard Switcher
Exec=${INSTALL_DIR}/${BIN}
X-GNOME-Autostart-enabled=true
EOF
  log "Autostart desktop entry: ${desktop_file}"
}

write_macos_autostart() {
  plist_dir="${HOME}/Library/LaunchAgents"
  plist_file="${plist_dir}/com.itmaxk.mx-keyboard-switcher.plist"
  mkdir -p "$plist_dir"
  cat >"$plist_file" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>com.itmaxk.mx-keyboard-switcher</string>
    <key>ProgramArguments</key>
    <array>
      <string>${INSTALL_DIR}/${BIN}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
      <key>SuccessfulExit</key>
      <false/>
    </dict>
  </dict>
</plist>
EOF
  launchctl unload "$plist_file" 2>/dev/null || true
  launchctl load -w "$plist_file"
  log "LaunchAgent: ${plist_file}"
  log "macOS: grant Accessibility permission (System Settings → Privacy & Security → Accessibility)."
}

configure_autostart() {
  os="$(uname -s)"
  case "$os" in
    Linux)
      write_linux_autostart
      if [ -n "${DISPLAY:-}" ]; then
        nohup setsid "${INSTALL_DIR}/${BIN}" >/dev/null 2>&1 &
        log "Started ${BIN} in the background."
      else
        log "DISPLAY is unset — skipped immediate start (console/Wayland session?)."
        log "Start manually after login: ${INSTALL_DIR}/${BIN}"
      fi
      ;;
    Darwin)
      write_macos_autostart
      ;;
    *)
      warn "autostart is not configured for OS: ${os}"
      ;;
  esac
}

print_done() {
  log ""
  log "Done."
  log "  Binary:  ${INSTALL_DIR}/${BIN}"
  log "  Run:     ${INSTALL_DIR}/${BIN}"
  if [ "$NO_AUTOSTART" -eq 0 ]; then
    os="$(uname -s)"
    case "$os" in
      Linux)
        log "  Disable autostart: rm -f ~/.config/autostart/${BIN}.desktop"
        ;;
      Darwin)
        log "  Disable autostart: launchctl unload ~/Library/LaunchAgents/com.itmaxk.mx-keyboard-switcher.plist && rm -f ~/Library/LaunchAgents/com.itmaxk.mx-keyboard-switcher.plist"
        ;;
    esac
  fi
}

main() {
  need_cmd curl
  detect_triplet
  SRC_BIN=""
  PREBUILT_TMP=""
  SOURCE_TMP=""

  if [ "$FROM_SOURCE" -eq 0 ] && [ -n "$TRIPLET" ]; then
    if [ -z "$VERSION" ]; then
      if tag="$(resolve_latest_tag)"; then
        VERSION="$tag"
      else
        warn "could not resolve latest release; falling back to source build"
        FROM_SOURCE=1
      fi
    fi
  else
    if [ -z "$TRIPLET" ] && [ "$FROM_SOURCE" -eq 0 ]; then
      warn "no prebuilt binary for $(uname -s)/$(uname -m); falling back to source build"
    fi
    FROM_SOURCE=1
  fi

  if [ "$FROM_SOURCE" -eq 0 ]; then
    if ! download_prebuilt "$VERSION"; then
      warn "prebuilt download failed for ${VERSION}/${TRIPLET}; falling back to source build"
      FROM_SOURCE=1
      if [ -n "${PREBUILT_TMP:-}" ]; then
        rm -rf "$PREBUILT_TMP"
        PREBUILT_TMP=""
      fi
      trap - EXIT INT HUP TERM
    fi
  fi

  if [ "$FROM_SOURCE" -eq 1 ]; then
    if ! command -v cargo >/dev/null 2>&1; then
      die "cargo not found. Install Rust via https://rustup.rs (curl https://sh.rustup.rs -sSf | sh) and re-run."
    fi
    build_from_source
  fi

  install_binary

  if [ "$NO_AUTOSTART" -eq 0 ]; then
    configure_autostart
  else
    log "Skipped autostart (--no-autostart)."
  fi

  print_done

  # Cleanup temp dirs if any.
  if [ -n "${PREBUILT_TMP:-}" ]; then
    rm -rf "$PREBUILT_TMP"
  fi
  if [ -n "${SOURCE_TMP:-}" ]; then
    rm -rf "$SOURCE_TMP"
  fi
  trap - EXIT INT HUP TERM
}

main
