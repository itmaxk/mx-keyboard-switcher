#!/usr/bin/env bash
# Run the x11_live integration tests on an isolated Xvfb display so the
# injected XTEST keystrokes never reach the real desktop (:10). Running them
# against the desktop display sprays phantom Shift+Tab / Ctrl+C / letters into
# whatever window has focus.
#
# Usage: scripts/run-x11-live-tests.sh [extra cargo test args...]
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v Xvfb >/dev/null; then
    echo "error: Xvfb is not installed (sudo apt install -y xvfb)" >&2
    exit 1
fi

# Pick a free display number.
display_num=99
while [ -e "/tmp/.X11-unix/X${display_num}" ]; do
    display_num=$((display_num + 1))
done
display=":${display_num}"

# -noreset: the server must not reset (dropping the uploaded keymap) when the
# last client — setxkbmap below — disconnects before the tests connect.
Xvfb "$display" -screen 0 1280x800x24 -nolisten tcp -noreset &
xvfb_pid=$!
trap 'kill "$xvfb_pid" 2>/dev/null || true' EXIT

# Wait for the server to accept connections.
for _ in $(seq 1 50); do
    if DISPLAY="$display" xset q >/dev/null 2>&1; then
        break
    fi
    sleep 0.1
done

# The tests need both us and ru layouts (see tests/x11_live.rs).
DISPLAY="$display" setxkbmap -layout us,ru

DISPLAY="$display" MXKS_TEST_DISPLAY="$display" \
    cargo test -p mxks-platform --test x11_live -- --ignored --test-threads=1 "$@"
