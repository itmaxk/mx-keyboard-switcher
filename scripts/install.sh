#!/bin/sh
# Install mx-keyboard-switcher from a GitHub Release, falling back to a
# local cargo build when no matching prebuilt asset exists.
set -eu

REPO="itmaxk/mx-keyboard-switcher"
BIN="mx-keyboard-switcher"
INSTALL_DIR="${HOME}/.local/bin"

XRDP_KEYBOARD_INI="/etc/xrdp/xrdp_keyboard.ini"
# Layout-switch shortcut written for xrdp when the session has none configured.
DEFAULT_GRP_OPTION="grp:alt_shift_toggle"

NO_AUTOSTART=0
FROM_SOURCE=0
NO_SYSTEM_CONFIG=0
VERSION=""

usage() {
  cat <<'EOF'
Usage: install.sh [options]

Options:
  --no-autostart   Install only; do not configure autostart or start new
                   instances (running instances are restarted on the new binary)
  --from-source    Skip prebuilt download; build with cargo
  --no-system-config
                   Linux: skip the keyboard layout checks and do not offer
                   to fix the xrdp keyboard config
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
    --no-system-config)
      NO_SYSTEM_CONFIG=1
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
  if [ -n "${RESTART_ENV_DIR:-}" ]; then
    rm -rf "$RESTART_ENV_DIR"
  fi
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
    Linux:aarch64|Linux:arm64)
      TRIPLET="aarch64-unknown-linux-gnu"
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

# Stop running instances of the installed binary and wait until they exit, so
# the new one does not lose the single-instance lock to the old process.
# Matches the full command line: Linux truncates process names to 15 chars,
# so `pkill -x mx-keyboard-switcher` never matches.
#
# On Linux each instance's environment is saved first (it vanishes from /proc
# with the process), so restart_stopped_instances can bring the new binary
# back on every X session, not only the one running the installer.
stop_running_instance() {
  pattern="${INSTALL_DIR}/${BIN}"
  uid="$(id -u)"
  pids="$(pgrep -u "$uid" -f "$pattern" 2>/dev/null)" || return 0
  if [ "$(uname -s)" = "Linux" ]; then
    if RESTART_ENV_DIR="$(mktemp -d)"; then
      for pid in $pids; do
        cat "/proc/${pid}/environ" >"${RESTART_ENV_DIR}/${pid}" 2>/dev/null ||
          rm -f "${RESTART_ENV_DIR}/${pid}"
      done
    else
      RESTART_ENV_DIR=""
      warn "could not save the running ${BIN} sessions; other X sessions get it after re-login."
    fi
  fi
  pkill -u "$uid" -f "$pattern" 2>/dev/null || return 0
  tries=0
  while pgrep -u "$uid" -f "$pattern" >/dev/null 2>&1; do
    tries=$((tries + 1))
    if [ "$tries" -eq 50 ]; then
      pkill -KILL -u "$uid" -f "$pattern" 2>/dev/null || true
    elif [ "$tries" -ge 70 ]; then
      warn "the running ${BIN} did not exit; the new version starts after re-login."
      return 0
    fi
    sleep 0.1
  done
}

install_binary() {
  [ -f "$SRC_BIN" ] || die "built binary not found: ${SRC_BIN}"
  mkdir -p "$INSTALL_DIR"
  stop_running_instance
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

# Start the new binary on every X display a stopped instance served, with that
# instance's environment (DISPLAY, XAUTHORITY, the D-Bus bus for the tray).
# Displays the installer already started on are skipped; the per-display lock
# would make a duplicate exit anyway.
restart_stopped_instances() {
  [ -n "${RESTART_ENV_DIR:-}" ] || return 0
  seen=" ${STARTED_DISPLAY:-} "
  for env_file in "$RESTART_ENV_DIR"/*; do
    [ -s "$env_file" ] || continue
    display="$(tr '\0' '\n' <"$env_file" | sed -n 's/^DISPLAY=//p' | head -n 1)"
    [ -n "$display" ] || continue
    case "$seen" in *" ${display} "*) continue ;; esac
    seen="${seen}${display} "
    # env -i replaces the environment with the saved one; the inner sh
    # detaches the daemon and prints its PID, so the env file can be removed
    # right after.
    # shellcheck disable=SC2016
    if pid="$( (cat "$env_file"; printf '%s\0' /bin/sh -c \
      'setsid "$0" </dev/null >/dev/null 2>&1 & echo "$!"' "${INSTALL_DIR}/${BIN}") |
      xargs -0 env -i)" && [ -n "$pid" ]; then
      STARTED_PIDS="${STARTED_PIDS} ${pid}:${display}"
      log "Restarted ${BIN} on display ${display}."
    else
      warn "could not restart ${BIN} on display ${display}; it starts there after re-login."
    fi
  done
  rm -rf "$RESTART_ENV_DIR"
  RESTART_ENV_DIR=""
}

# A daemon that cannot open its X display (the session ended, XAUTHORITY
# changed) or finds another instance there exits within milliseconds, so a
# moment later each started PID tells whether it really runs. A start made
# directly by this shell stays a zombie until reaped, hence the state check.
verify_started_instances() {
  [ -n "$STARTED_PIDS" ] || return 0
  sleep 1
  for entry in $STARTED_PIDS; do
    pid="${entry%%:*}"
    display="${entry#*:}"
    state="$(ps -o stat= -p "$pid" 2>/dev/null)" || state=""
    case "$state" in
      "" | Z*)
        warn "${BIN} exited right after starting on display ${display}; it starts there after re-login."
        ;;
    esac
  done
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

# Ask a yes/no question on the terminal (stdin may be the `curl | sh` pipe).
# Defaults to "no" when there is no terminal to ask on.
confirm() {
  (exec </dev/tty) 2>/dev/null || return 1
  printf '%s [y/N] ' "$1" >/dev/tty
  read -r answer </dev/tty || return 1
  case "$answer" in
    [yY]*) return 0 ;;
    *) return 1 ;;
  esac
}

# Layout list and group-switch option of the running X session.
read_session_keyboard() {
  SESSION_LAYOUTS=""
  SESSION_GRP=""
  [ -n "${DISPLAY:-}" ] && command -v setxkbmap >/dev/null 2>&1 || return 0
  query="$(setxkbmap -query 2>/dev/null)" || return 0
  SESSION_LAYOUTS="$(printf '%s\n' "$query" | sed -n 's/^layout:[[:space:]]*//p')"
  SESSION_GRP="$(printf '%s\n' "$query" | sed -n 's/^options:[[:space:]]*//p' |
    tr ',' '\n' | grep '^grp:' | head -n 1)" || true
}

check_session_layouts() {
  [ -n "$SESSION_LAYOUTS" ] || return 0
  has_us=0
  has_ru=0
  case ",${SESSION_LAYOUTS}," in *,us,*) has_us=1 ;; esac
  case ",${SESSION_LAYOUTS}," in *,ru,*) has_ru=1 ;; esac
  [ "$has_us" -eq 1 ] && [ "$has_ru" -eq 1 ] && return 0
  warn "the X keyboard layouts are '${SESSION_LAYOUTS}', but MX Keyboard Switcher needs both us and ru."
  warn "add the missing layout in your desktop keyboard settings (or: setxkbmap -layout us,ru)."
}

# Print the xrdp keyboard config patched so that every RDP (re)connect loads
# both us and ru. xrdp replaces the X keymap with the client's single layout on
# each connect, which silently drops the second layout and the switch shortcut.
#
# - every layouts map that sends a us or ru client to a single layout is
#   pointed at us,ru instead;
# - a [rdp_keyboard_mxks] section is appended for standard PC keyboards
#   (type 4; xrdp reads subtype 0 as 1) carrying the switch shortcut. xrdp
#   ignores model/options in [default], and a later section with the same
#   keyboard_type resets an earlier match, so it must be the last section.
#   Its layouts map is patched too, even if no other section uses it yet.
patch_xrdp_keyboard() {
  awk -v grp="$2" -v mxks_map="default_layouts_map" '
    function section_name(line) {
      sub(/^[[:space:]]*\[/, "", line)
      sub(/\][[:space:]]*$/, "", line)
      return line
    }
    function value(line) {
      sub(/^[^=]*=[[:space:]]*/, "", line)
      sub(/[[:space:]]*$/, "", line)
      return line
    }
    { sub(/\r$/, "") }
    NR == FNR {
      if ($0 ~ /^[[:space:]]*\[/) {
        sec = section_name($0)
        if (sec == "rdp_keyboard_mxks") ours = 1
      } else if ($0 ~ /^[[:space:]]*layouts_map[[:space:]]*=/) {
        maps[value($0)] = 1
      }
      next
    }
    FNR == 1 && !ours { maps[mxks_map] = 1 }
    /^[[:space:]]*\[/ { sec = section_name($0) }
    sec in maps && /^[[:space:]]*rdp_layout_(us|ru)[[:space:]]*=/ {
      v = value($0)
      if (v == "us" || v == "ru") {
        key = $0
        sub(/[[:space:]]*=.*/, "", key)
        $0 = key "=us,ru"
      }
    }
    { print }
    END {
      if (!ours) {
        print ""
        print "; Added by mx-keyboard-switcher: keep both us and ru (and the layout"
        print "; switch shortcut) after every RDP connect. Must stay the last section."
        print "[rdp_keyboard_mxks]"
        print "keyboard_type=4"
        print "keyboard_subtype=1"
        print "model=pc105"
        print "options=" grp
        print "rdp_layouts=default_rdp_layouts"
        print "layouts_map=" mxks_map
      }
    }
  ' "$1" "$1"
}

check_xrdp_keyboard() {
  [ -f "$XRDP_KEYBOARD_INI" ] || return 0
  # This check is optional: any failure below must not abort the install.
  if [ ! -r "$XRDP_KEYBOARD_INI" ]; then
    warn "cannot read ${XRDP_KEYBOARD_INI}; skipped the xrdp keyboard check."
    return 0
  fi
  if ! command -v diff >/dev/null 2>&1; then
    warn "xrdp detected but 'diff' is missing; skipped the xrdp keyboard check."
    return 0
  fi
  grp="${SESSION_GRP:-$DEFAULT_GRP_OPTION}"
  if ! patched="$(mktemp)"; then
    warn "could not create a temp file; skipped the xrdp keyboard check."
    return 0
  fi
  if ! patch_xrdp_keyboard "$XRDP_KEYBOARD_INI" "$grp" >"$patched"; then
    rm -f "$patched"
    warn "could not patch a copy of ${XRDP_KEYBOARD_INI}; skipped the xrdp keyboard check."
    return 0
  fi
  if diff -q "$XRDP_KEYBOARD_INI" "$patched" >/dev/null; then
    rm -f "$patched"
    log "xrdp keyboard config already keeps us,ru."
    return 0
  fi

  log ""
  log "xrdp is installed. On every RDP connect it reloads the keyboard with only"
  log "the client's layout, so the us/ru switch stops working until restart."
  log "Proposed change to ${XRDP_KEYBOARD_INI} (switch shortcut: ${grp}):"
  diff -u "$XRDP_KEYBOARD_INI" "$patched" || true
  if confirm "Apply it with sudo (a .bak copy is kept)?"; then
    # cp onto the existing file keeps its owner and mode.
    if sudo cp -p "$XRDP_KEYBOARD_INI" "${XRDP_KEYBOARD_INI}.bak" &&
      sudo cp "$patched" "$XRDP_KEYBOARD_INI"; then
      log "Updated ${XRDP_KEYBOARD_INI}; reconnect RDP to apply."
    else
      warn "could not update ${XRDP_KEYBOARD_INI}; the layout switch may break after RDP reconnects."
    fi
  else
    log "Left ${XRDP_KEYBOARD_INI} unchanged. To apply later, re-run the installer in a terminal."
  fi
  rm -f "$patched"
}

configure_linux_keyboard() {
  read_session_keyboard
  check_session_layouts
  check_xrdp_keyboard
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
        STARTED_PIDS="${STARTED_PIDS} $!:${DISPLAY}"
        STARTED_DISPLAY="$DISPLAY"
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
  RESTART_ENV_DIR=""
  STARTED_DISPLAY=""
  STARTED_PIDS=""

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

  if [ "$(uname -s)" = "Linux" ]; then
    if [ "$NO_SYSTEM_CONFIG" -eq 0 ]; then
      configure_linux_keyboard
    else
      log "Skipped keyboard layout checks (--no-system-config)."
    fi
  fi

  if [ "$NO_AUTOSTART" -eq 0 ]; then
    configure_autostart
  else
    log "Skipped autostart (--no-autostart)."
  fi
  restart_stopped_instances
  verify_started_instances

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
