#!/usr/bin/env bash
# Installed beside QuixiDesk; never depend on the source checkout or caller's cwd.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: tscdesk [--check | --help] [application arguments...]
Launch TSCDesk in the existing graphical session. --check diagnoses without opening it.
EOF
}
case "${1:-}" in
    --help|-h) usage; exit 0 ;;
esac
fail() { printf 'tscdesk: %s\n' "$*" >&2; exit 1; }
(( EUID != 0 )) || fail 'Run as your desktop user, not root.'
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
export QUIXI_RESOURCES_DIR="${QUIXI_RESOURCES_DIR-$ROOT/resources}"
# A test profile must keep its own workspace; explicit user overrides still win.
if [[ ! ${QUIXI_WORKSPACE+x} ]]; then
    PROFILE_REQUESTED=0
    if [[ -n "${QUIXI_TEST_PROFILE:-}" ]]; then PROFILE_REQUESTED=1; fi
    for arg in "$@"; do
        if [[ "$arg" == --test-profile ]]; then PROFILE_REQUESTED=1; fi
    done
    if (( ! PROFILE_REQUESTED )); then export QUIXI_WORKSPACE="$HOME"; fi
fi
export PATH="$ROOT/bin:$PATH"
[[ -n "${WAYLAND_DISPLAY:-}" || -n "${DISPLAY:-}" ]] || fail 'No graphical session: DISPLAY and WAYLAND_DISPLAY are unset. Open a Crostini desktop terminal.'
[[ -x "$ROOT/bin/QuixiDesk" ]] || fail "Missing executable: $ROOT/bin/QuixiDesk"
[[ -f "$QUIXI_RESOURCES_DIR/ui/index.html" ]] || fail "Missing UI resources: $QUIXI_RESOURCES_DIR/ui/index.html"

# Some Crostini terminals omit the address while the user's session bus exists.
# Reuse it rather than isolating the app from the login keyring/session services.
if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" && -n "${XDG_RUNTIME_DIR:-}" && -S "$XDG_RUNTIME_DIR/bus" ]]; then
    command -v python3 >/dev/null 2>&1 || fail 'Missing python3 (needed to encode the session bus path).'
    BUS_PATH="$(python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe="/.-_*"))' "$XDG_RUNTIME_DIR/bus")"
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$BUS_PATH"
fi

# Only query an existing session. Never create or unlock a keyring, or store a password.
if [[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]] && command -v dbus-send >/dev/null 2>&1; then
    if ! dbus-send --session --print-reply --reply-timeout=1500 --dest=org.freedesktop.DBus /org/freedesktop/DBus \
        org.freedesktop.DBus.NameHasOwner string:org.freedesktop.secrets 2>/dev/null | grep -q 'boolean true'; then
        printf '%s\n' 'tscdesk: warning: Secret Service is unavailable; unlock your login keyring with Seahorse.' >&2
    fi
else
    printf '%s\n' 'tscdesk: warning: Secret Service could not be checked; unlock your login keyring with Seahorse if needed.' >&2
fi
if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
    command -v dbus-run-session >/dev/null 2>&1 || fail 'Missing dbus-run-session (install dbus-user-session).'
fi
if [[ "${1:-}" == --check ]]; then
    printf 'TSCDesk launcher OK\nResources: %s\nWorkspace: %s\n' "$QUIXI_RESOURCES_DIR" "${QUIXI_WORKSPACE:-selected by application/test profile}"
    if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
        printf '%s\n' 'D-Bus: a private session will be started for the application.'
    else
        printf '%s\n' 'D-Bus: existing session preserved.'
    fi
    exit 0
fi
if [[ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
    # Execute the app directly, not this launcher: at most one D-Bus session.
    exec dbus-run-session -- "$ROOT/bin/QuixiDesk" "$@"
fi
exec "$ROOT/bin/QuixiDesk" "$@"
