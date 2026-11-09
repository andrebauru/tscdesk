#!/usr/bin/env bash
# User-only Debian 12+/Ubuntu 22.04+ (including ChromeOS Crostini) installer.
set -euo pipefail
SOURCE_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
usage() {
    cat <<'EOF'
Usage: scripts/install-linux.sh [options]
  --help, -h       Show this help (also permitted as root)
  --skip-deps      Do not run apt; dependencies must already be installed
  --skip-build     Package existing target/release binaries; do not run Rust
  --reinstall      Replace prior managed releases after the new release is ready
  --jobs N         Cargo jobs (default: CARGO_BUILD_JOBS, otherwise 2)
  --prefix PATH    Absolute user-owned install path (default: ~/.local/opt/tscdesk)
  --yes            Consent to sudo apt and Rust stable installation/update

Supports x86_64 and aarch64; ARM support is not a claim of ARM testing.
Installs versioned releases, ~/.local/bin/tscdesk and an XDG desktop entry.
Existing paths must carry this installer's managed marker. Normal updates keep old
releases; --reinstall removes only prior managed releases after a successful switch.
~/.quixi/MCP configuration is never removed or overwritten. Optional bubblewrap
is not installed automatically; WebKit sandbox and input permissions are unchanged.
EOF
}
# Help must remain usable even as root or without platform dependencies.
for arg in "$@"; do
    case "$arg" in --help|-h) usage; exit 0 ;; esac
done
fail() { printf 'install-linux: %s\n' "$*" >&2; exit 1; }
(( EUID != 0 )) || fail 'Run as your desktop user, not root (sudo is used only for apt).'
SKIP_DEPS=0 SKIP_BUILD=0 YES=0 REINSTALL=0
JOBS="${CARGO_BUILD_JOBS:-2}"
PREFIX="$HOME/.local/opt/tscdesk"
while (( $# )); do
    case "$1" in
        --skip-deps) SKIP_DEPS=1; shift ;;
        --skip-build) SKIP_BUILD=1; shift ;;
        --reinstall) REINSTALL=1; shift ;;
        --yes) YES=1; shift ;;
        --jobs|--prefix)
            (( $# >= 2 )) || fail "Missing value for $1"
            if [[ "$1" == --jobs ]]; then JOBS="$2"; else PREFIX="$2"; fi
            shift 2 ;;
        *) fail "Unknown option: $1 (see --help)" ;;
    esac
done
[[ "$JOBS" =~ ^[1-9][0-9]*$ ]] || fail '--jobs/CARGO_BUILD_JOBS must be a positive integer.'
[[ "$PREFIX" == /* && "$PREFIX" != / ]] || fail '--prefix must be an absolute path other than /.'
[[ "$(uname -s)" == Linux ]] || fail 'This installer requires Linux.'
case "$(uname -m)" in x86_64|aarch64) ;; *) fail 'Supported architectures: x86_64 and aarch64.' ;; esac
# OS validation is independent of --skip-deps; no untrusted environment override.
[[ -r /etc/os-release ]] || fail 'Missing /etc/os-release.'
# Runtime distribution metadata is provided by the host, not the source checkout.
# shellcheck source=/dev/null
. /etc/os-release
case "${ID:-}:${VERSION_ID:-}" in
    debian:*) MIN_VERSION=12 ;;
    ubuntu:*) MIN_VERSION=22.04 ;;
    *) fail 'Requires Debian 12+ or Ubuntu 22.04+.' ;;
esac
dpkg --compare-versions "$VERSION_ID" ge "$MIN_VERSION" || fail "Requires $ID $MIN_VERSION or newer."
# Preflight and publishing share the same validator, including ancestor symlinks.
# No source-root environment override: tests copy these scripts into a fixture tree.
manage_install() {
    python3 - "$1" "$PREFIX" "$SOURCE_ROOT" "${2:-}" <<'PY'
import contextlib
import fcntl
import hashlib
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import sys
import tempfile
import time

mode, raw_prefix, source, stage = sys.argv[1:]
MARK = 'TSCDesk Linux installer managed v1\n'
prefix = Path(os.path.abspath(raw_prefix))
home = Path(os.environ['HOME'])
xdg = Path(os.environ.get('XDG_DATA_HOME') or str(home / '.local/share'))
launcher = home / '.local/bin/tscdesk'
desktop = xdg / 'applications/tscdesk.desktop'
identity = hashlib.sha256(str(prefix).encode()).hexdigest()
launcher_mark = '# TSCDesk-managed-v1 ' + identity
desktop_mark = 'X-TSCDesk-Managed-v1=' + identity

def fail(message):
    raise RuntimeError(message)

def safe_path(path):
    if not path.is_absolute() or any(ord(c) < 32 or ord(c) == 127 for c in str(path)):
        fail('Path must be absolute and contain no control characters: ' + str(path))
    for part in [path, *path.parents]:
        if part.is_symlink():
            fail('Refusing symlink path: ' + str(part))
        if part.exists() and part.stat().st_uid != os.getuid() and part == path:
            fail('Path is not owned by this user: ' + str(part))

def marker(path):
    return not path.is_symlink() and path.is_file() and path.read_text() == MARK

def validate():
    # Inspect the original spelling too: normalization must not hide symlink/../ paths.
    safe_path(Path(raw_prefix))
    for path in (prefix, launcher, desktop):
        safe_path(path)
    if prefix.exists():
        if not prefix.is_dir() or not marker(prefix / '.tscdesk-managed'):
            fail('Unmanaged prefix; refusing to overwrite: ' + str(prefix))
        safe_path(prefix / 'releases')
        if (prefix / 'releases').exists() and not (prefix / 'releases').is_dir():
            fail('Invalid releases directory')
        current = prefix / 'current'
        if os.path.lexists(current):
            if not current.is_symlink():
                fail('current must be a managed release symlink')
            target = os.readlink(current)
            if (not re.fullmatch(r'releases/[A-Za-z0-9_.-]+', target)
                    or target.split('/')[-1] in ('.', '..')):
                fail('Refusing unmanaged current target')
            release = prefix / target
            safe_path(release)
            if not marker(release / '.tscdesk-release'):
                fail('current target has no managed release marker')
    for path, expected in ((launcher, launcher_mark), (desktop, desktop_mark)):
        if path.exists() and (not path.is_file() or expected not in path.read_text().splitlines()):
            fail('Unmanaged launcher/desktop conflict: ' + str(path))

# General desktop strings unescape once; Exec then unescapes its quoted argument.
def desktop_string(value):
    return value.replace('\\', '\\\\').replace('\n', '\\n').replace('\r', '\\r').replace('\t', '\\t').replace(' ', '\\s')

def desktop_exec(value):
    quoted = ''.join('\\' + c if c in '\\"$`' else c for c in value.replace('%', '%%'))
    return '"' + quoted.replace('\\', '\\\\') + '"'

def atomic_file(path, data, perms):
    fd, name = tempfile.mkstemp(prefix='.tscdesk-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as out:
            out.write(data)
            out.flush()
            os.fsync(out.fileno())
        os.chmod(name, perms)
        os.replace(name, path)
    finally:
        if os.path.lexists(name):
            os.unlink(name)

def stop_managed_processes():
    """Stops only this user's executables running from managed release folders."""
    releases = prefix / 'releases'
    if not releases.is_dir():
        return
    parents = {}
    roots = set()
    for proc in Path('/proc').iterdir():
        if not proc.name.isdigit():
            continue
        try:
            if proc.stat().st_uid != os.getuid():
                continue
            pid = int(proc.name)
            fields = (proc / 'stat').read_text().rsplit(')', 1)[1].split()
            parents[pid] = int(fields[1])
            executable = (proc / 'exe').resolve(strict=True)
            if releases in executable.parents:
                roots.add(pid)
        except (FileNotFoundError, PermissionError, ProcessLookupError, OSError):
            continue
    targets = set(roots)
    changed = True
    while changed:
        before = len(targets)
        targets.update(pid for pid, parent in parents.items() if parent in targets)
        changed = len(targets) != before
    for pid in targets:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    def still_running(pid):
        try:
            fields = Path('/proc', str(pid), 'stat').read_text().rsplit(')', 1)[1].split()
            return fields[0] != 'Z'
        except (FileNotFoundError, PermissionError, OSError):
            return False
    deadline = time.monotonic() + 5
    remaining = list(targets)
    while remaining and time.monotonic() < deadline:
        remaining = [pid for pid in remaining if still_running(pid)]
        if remaining:
            time.sleep(0.1)
    for pid in remaining:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

def remove_old_managed_releases(current):
    """Removes prior releases only when their installer marker is intact."""
    releases = prefix / 'releases'
    for old in releases.iterdir():
        if old == current or old.is_symlink() or not old.is_dir():
            continue
        if marker(old / '.tscdesk-release'):
            try:
                shutil.rmtree(old)
            except OSError as exc:
                print('install-linux: warning: active release was installed, but the old managed release '
                      f'{old} could not be removed: {exc}', file=sys.stderr)

try:
    validate()
    if mode == 'preflight':
        sys.exit(0)
    # All artifacts are prepared before creating or modifying installed files.
    if mode not in ('publish', 'reinstall'):
        fail('Unknown installation mode')
    staged = Path(stage)
    icon = next((name for name in ('quixi-mark.svg', 'quixi-logo.svg')
                 if (staged / 'resources' / name).is_file()), None)
    if not icon:
        fail('Missing resources/quixi-mark.svg or quixi-logo.svg')
    version_match = re.search(r'^version\s*=\s*"([A-Za-z0-9_.-]+)"',
                              (Path(source) / 'Cargo.toml').read_text(), re.M)
    version = version_match.group(1) if version_match else 'unknown'
    wrapper = ('#!/usr/bin/env bash\n' + launcher_mark + '\nset -euo pipefail\nexec ' +
               shlex.quote(str(prefix / 'current/bin/launch-linux.sh')) + ' "$@"\n').encode()
    # A fixed executable also allows '=' in the script path (forbidden in an Exec executable).
    entry = ('[Desktop Entry]\nType=Application\nName=TsCIDE\n' + desktop_mark +
             '\nExec=/bin/bash ' + desktop_exec(str(launcher)) + '\nIcon=' +
             desktop_string(str(prefix / 'current/resources' / icon)) +
             '\nTerminal=false\nStartupWMClass=QuixiDesk\nCategories=Development;\n').encode()
    prefix.mkdir(parents=True, exist_ok=True)
    # Directory locking avoids lock-file symlinks and serializes managed updates.
    with contextlib.ExitStack() as stack:
        lock = os.open(prefix, os.O_RDONLY | os.O_DIRECTORY)
        stack.callback(os.close, lock)
        fcntl.flock(lock, fcntl.LOCK_EX)
        if not (prefix / '.tscdesk-managed').exists():
            # Only the just-created empty directory may become managed.
            if any(prefix.iterdir()):
                fail('Prefix changed during installation')
            atomic_file(prefix / '.tscdesk-managed', MARK.encode(), 0o644)
        validate()
        releases = prefix / 'releases'
        releases.mkdir(exist_ok=True)
        release = Path(tempfile.mkdtemp(prefix=version + '-', dir=releases))
        # Never clean a prior release or arbitrary user data on failure.
        shutil.copytree(staged, release, dirs_exist_ok=True)
        atomic_file(release / '.tscdesk-release', MARK.encode(), 0o644)
        launcher.parent.mkdir(parents=True, exist_ok=True)
        desktop.parent.mkdir(parents=True, exist_ok=True)
        validate()
        previous = {p: (p.read_bytes(), p.stat().st_mode & 0o777) if p.exists() else None
                    for p in (launcher, desktop)}
        link = prefix / ('.current-' + release.name)
        try:
            if mode == 'reinstall':
                stop_managed_processes()
            atomic_file(launcher, wrapper, 0o755)
            atomic_file(desktop, entry, 0o644)
            os.symlink('releases/' + release.name, link)
            os.replace(link, prefix / 'current')  # Commit point; old release stays intact.
        except BaseException:
            for path, old in previous.items():
                if old is None:
                    if path.exists():
                        path.unlink()
                else:
                    atomic_file(path, *old)
            raise
        finally:
            if link.is_symlink():
                link.unlink()
        if mode == 'reinstall':
            remove_old_managed_releases(release)
    print('Installed TsCIDE at ' + str(prefix))
    print('Launch: ' + str(launcher) + ' (add ~/.local/bin to PATH if necessary)')
except (OSError, ValueError, RuntimeError) as exc:
    print('install-linux: ' + str(exc), file=sys.stderr)
    sys.exit(1)
PY
}
confirm() {
    local answer
    printf 'install-linux: %s\n' "$1" >&2
    if (( YES )); then return; fi
    [[ -t 0 ]] || fail 'Interactive consent required; review --help and use --yes to consent.'
    read -r -p 'Continue? [y/N] ' answer
    [[ "$answer" == y || "$answer" == Y || "$answer" == yes ]] || fail 'Cancelled.'
}
install_dependencies() {
    command -v sudo >/dev/null 2>&1 || fail 'sudo is required for apt (or use --skip-deps).'
    confirm 'Will use sudo apt-get update/install for system dependencies. No input permission or sandbox changes.'
    sudo apt-get update
    sudo apt-get install --no-install-recommends -y build-essential pkg-config libgtk-3-dev \
        libwebkit2gtk-4.1-dev curl git ca-certificates perl python3 dbus-user-session \
        gnome-keyring zenity libsecret-tools xdg-utils
}

# Adds one controlled PATH line without replacing shell configuration.
configure_user_path() {
    local shell_name config line
    shell_name="$(basename -- "${SHELL:-bash}")"
    case "$shell_name" in
        bash) config="$HOME/.bashrc" ;;
        zsh) config="$HOME/.zshrc" ;;
        *) config="$HOME/.profile" ;;
    esac
    line='export PATH="$HOME/.local/bin:$PATH"'
    if [[ -L "$config" ]]; then
        printf 'install-linux: warning: PATH was not changed because %s is a symlink.\n' "$config" >&2
        return
    fi
    if [[ -e "$config" && ! -f "$config" ]]; then
        printf 'install-linux: warning: PATH was not changed because %s is not a regular file.\n' "$config" >&2
        return
    fi
    if [[ -e "$config" && "$(stat -c %u -- "$config")" != "$EUID" ]]; then
        printf 'install-linux: warning: PATH was not changed because %s belongs to another user.\n' "$config" >&2
        return
    fi
    if [[ ! -f "$config" ]] || ! grep -Fqx -- "$line" "$config"; then
        echo "$line" >> "$config"
        printf 'install-linux: added ~/.local/bin to PATH in %s\n' "$config"
    else
        printf 'install-linux: PATH is already configured in %s\n' "$config"
    fi
    export PATH="$HOME/.local/bin:$PATH"
    printf 'install-linux: open a new Terminal or run: source %q\n' "$config"
}

# Marks the next launch without following user-controlled symlinks.
request_master_password_setup() {
    python3 - "$HOME/.quixi" <<'PY'
import os
import sys
from pathlib import Path

directory = Path(sys.argv[1])
if directory.is_symlink():
    raise SystemExit('install-linux: refusing symlinked credential data directory')
directory.mkdir(mode=0o700, parents=True, exist_ok=True)
if not directory.is_dir() or directory.stat().st_uid != os.getuid():
    raise SystemExit('install-linux: credential data directory is not owned by the current user')
marker = directory / 'master-password-setup-required'
if marker.is_symlink():
    raise SystemExit('install-linux: refusing symlinked master-password marker')
temporary = directory / f'.master-password-setup-{os.getpid()}.tmp'
fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
try:
    os.write(fd, b'Set a new master password after reinstalling TsCIDE.\n')
    os.fsync(fd)
finally:
    os.close(fd)
os.replace(temporary, marker)
PY
    printf 'install-linux: TsCIDE will request a new master password on first launch.\n'
}
DEPS_INSTALLED=0
if ! command -v python3 >/dev/null 2>&1; then
    (( ! SKIP_DEPS )) || fail 'python3 is required; rerun without --skip-deps so it can be installed.'
    # A pristine Crostini image may not have Python. Install the complete dependency
    # set first, then run the same path-safety preflight used on configured systems.
    install_dependencies
    DEPS_INSTALLED=1
fi
command -v python3 >/dev/null 2>&1 || fail 'python3 is still unavailable after dependency installation.'
manage_install preflight
if (( ! SKIP_DEPS && ! DEPS_INSTALLED )); then
    install_dependencies
fi
TEMP_STAGE="$(mktemp -d)"
trap 'rm -rf -- "$TEMP_STAGE"' EXIT
if (( ! SKIP_BUILD )); then
    export PATH="$HOME/.cargo/bin:$PATH"
    if command -v rustup >/dev/null 2>&1; then
        # An installed "stable" can be years old; existence is not a version check.
        confirm 'Will install/update Rust stable using rustup; your default toolchain will not change.'
    else
        confirm 'Will download official rustup over TLS and install current Rust stable alongside any system Rust (no shell/default changes).'
        curl --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --show-error --location \
            https://sh.rustup.rs -o "$TEMP_STAGE/rustup-init.sh"
        # Keep apt/manual Rust untouched and avoid selecting a new default or editing shell files.
        RUSTUP_INIT_SKIP_PATH_CHECK=yes bash "$TEMP_STAGE/rustup-init.sh" -y --profile minimal --default-toolchain none --no-modify-path
    fi
    rustup toolchain install stable --profile minimal
    CARGO=(cargo +stable)
    "${CARGO[@]}" build --manifest-path "$SOURCE_ROOT/Cargo.toml" --target-dir "$SOURCE_ROOT/target" \
        --locked --release --workspace --jobs "$JOBS"
fi
[[ -x "$SOURCE_ROOT/target/release/QuixiDesk" ]] || fail 'Missing executable target/release/QuixiDesk; build first.'
mkdir -p "$TEMP_STAGE/payload/bin"
cp -L -- "$SOURCE_ROOT/target/release/QuixiDesk" "$TEMP_STAGE/payload/bin/QuixiDesk"
for helper in quixi-mcp-builtin quixi-chrome-native-host quixi-ssh-askpass quixi-disclaimer quixi-guest-daemon quixi-mcp-worker; do
    if [[ -f "$SOURCE_ROOT/target/release/$helper" ]]; then
        cp -L -- "$SOURCE_ROOT/target/release/$helper" "$TEMP_STAGE/payload/bin/$helper"
        chmod +x "$TEMP_STAGE/payload/bin/$helper"
    fi
done
cp -- "$SOURCE_ROOT/scripts/launch-linux.sh" "$TEMP_STAGE/payload/bin/launch-linux.sh"
chmod +x "$TEMP_STAGE/payload/bin/launch-linux.sh"
# Dereference source links to keep the installation independent of the checkout.
cp -RL -- "$SOURCE_ROOT/resources" "$TEMP_STAGE/payload/resources"
for license in LICENSE LICENCE; do
    if [[ -f "$SOURCE_ROOT/$license" ]]; then cp -L -- "$SOURCE_ROOT/$license" "$TEMP_STAGE/payload/$license"; fi
done
if (( REINSTALL )); then
    manage_install reinstall "$TEMP_STAGE/payload"
    request_master_password_setup
else
    manage_install publish "$TEMP_STAGE/payload"
fi
configure_user_path
