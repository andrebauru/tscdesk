"""Isolated Linux installer/launcher contracts; no apt, Rust or GUI required.

Run: python3 -m unittest discover -s tests/install -p test_linux_install.py -v
The scripts are copied into a fake source tree; no source-root injection switch exists.
"""
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import socket
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]
LINUX = platform.system() == "Linux"


@unittest.skipUnless(LINUX, "requires Linux bash")
class HelpTests(unittest.TestCase):
    def test_syntax_and_help(self):
        for name in ("install-linux.sh", "launch-linux.sh"):
            script = REPO / "scripts" / name
            subprocess.run(["bash", "-n", str(script)], check=True)
            result = subprocess.run(["bash", str(script), "--help"], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Usage:", result.stdout)

    @unittest.skipUnless(hasattr(os, "geteuid") and os.geteuid() == 0, "root-only rejection test")
    def test_root_rejected(self):
        for name in ("install-linux.sh", "launch-linux.sh"):
            result = subprocess.run(["bash", str(REPO / "scripts" / name)], capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("not root", result.stderr)


@unittest.skipUnless(LINUX and hasattr(os, "geteuid") and os.geteuid() != 0,
                     "installation fixtures require an unprivileged Linux user")
class LinuxInstallTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="tscdesk-tests-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "fake source"
        self.home = self.root / 'home space $cash `tick` "quote" %percent \\backslash =equal'
        self.prefix = self.home / "opt/tscdesk space"
        self.bin = self.root / "commands"
        for path in (self.source / "scripts", self.source / "target/release",
                     self.source / "resources/ui", self.home, self.bin):
            path.mkdir(parents=True)
        for name in ("install-linux.sh", "launch-linux.sh"):
            shutil.copyfile(REPO / "scripts" / name, self.source / "scripts" / name)
        (self.source / "Cargo.toml").write_text('[workspace.package]\nversion = "1.2.3"\n')
        (self.source / "LICENCE").write_text("fixture licence\n")
        (self.source / "resources/quixi-logo.svg").write_text("<svg/>\n")
        (self.source / "resources/quixi-mark.svg").write_text("<svg/>\n")
        (self.source / "resources/ui/index.html").write_text("fixture UI\n")
        app = self.source / "target/release/TsCIDE"
        app.write_text('''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
Path(os.environ['CAPTURE']).write_text(json.dumps({'args': sys.argv[1:], 'env': dict(os.environ)}))
''')
        app.chmod(0o755)
        for name in ("quixi-mcp-builtin", "quixi-chrome-native-host", "quixi-ssh-askpass"):
            helper = app.parent / name
            helper.write_text("#!/bin/sh\nexit 0\n")
            helper.chmod(0o755)
        self.capture = self.root / "capture.json"
        self.forbidden = self.root / "forbidden.log"
        self.dbus_log = self.root / "dbus.log"
        self.keyring_log = self.root / "keyring.log"
        self.env = os.environ.copy()
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "QUIXI_WORKSPACE",
                     "QUIXI_RESOURCES_DIR", "QUIXI_TEST_PROFILE", "CARGO_BUILD_JOBS", "XDG_DATA_HOME",
                     "XDG_RUNTIME_DIR"):
            self.env.pop(name, None)
        self.env.update(HOME=str(self.home), XDG_DATA_HOME=str(self.home / "xdg data"),
                        SHELL="/bin/bash",
                        PATH=str(self.bin) + ":" + os.environ["PATH"], CAPTURE=str(self.capture),
                        FORBIDDEN=str(self.forbidden), DBUS_LOG=str(self.dbus_log),
                        KEYRING_LOG=str(self.keyring_log), TMPDIR=str(self.root))
        for name in ("sudo", "apt-get", "cargo", "rustup", "curl"):
            self.command(name, '#!/bin/sh\necho "$0 $*" >> "$FORBIDDEN"\nexit 99\n')
        self.command("dbus-send", "#!/bin/sh\necho 'boolean true'\n")
        self.command("gnome-keyring-daemon", '#!/bin/sh\nprintf "%s\\n" "$*" >> "$KEYRING_LOG"\n')
        self.command("dbus-run-session", '''#!/bin/sh
echo once >> "$DBUS_LOG"
[ "$1" = -- ] || exit 97
shift
export DBUS_SESSION_BUS_ADDRESS=fixture:private
exec "$@"
''')
        # Data never owned by the installer, including MCP configuration.
        (self.home / ".quixi").mkdir()
        (self.home / ".quixi/data").write_text("keep me")
        (self.home / ".gemini").mkdir()
        (self.home / ".gemini/mcp.json").write_text("keep MCP")

    def tearDown(self):
        self.assertFalse(self.forbidden.exists(), "apt/Rust must never run in fixture tests")

    def command(self, name, content):
        path = self.bin / name
        path.write_text(content)
        path.chmod(0o755)

    def install(self, *args, ok=True, env=None):
        result = subprocess.run(["bash", str(self.source / "scripts/install-linux.sh"),
                                 "--skip-deps", "--skip-build", "--prefix", str(self.prefix), *args],
                                cwd=self.root, env=env or self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, ok, result.stdout + result.stderr)
        return result

    @property
    def launcher(self):
        return self.home / ".local/bin/tscdesk"

    @property
    def desktop(self):
        return self.home / "xdg data/applications/tscdesk.desktop"

    def launch(self, *args, ok=True, **updates):
        env = self.env.copy()
        env.update(updates)
        result = subprocess.run([str(self.launcher), *args], cwd=self.root,
                                env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, ok, result.stdout + result.stderr)
        return result

    def test_reinstall_preserves_releases_and_user_data(self):
        self.install()
        old = self.prefix / os.readlink(self.prefix / "current")
        (old / "user-data").write_text("preserved")
        (self.prefix / "personal-data").write_text("preserved too")
        self.install()
        self.assertNotEqual(old, (self.prefix / "current").resolve())
        self.assertEqual((old / "user-data").read_text(), "preserved")
        self.assertEqual((self.prefix / "personal-data").read_text(), "preserved too")
        self.assertEqual((self.home / ".quixi/data").read_text(), "keep me")
        self.assertEqual((self.home / ".gemini/mcp.json").read_text(), "keep MCP")
        self.assertEqual((self.prefix / "current/LICENCE").read_text(), "fixture licence\n")
        for name in ("quixi-mcp-builtin", "quixi-chrome-native-host", "quixi-ssh-askpass"):
            self.assertTrue((self.prefix / "current/bin" / name).is_file())
        shutil.rmtree(self.source)
        self.launch("from detached install", DISPLAY=":0", DBUS_SESSION_BUS_ADDRESS="fixture:existing")

    def test_explicit_reinstall_replaces_only_managed_releases_and_keeps_account_data(self):
        self.install()
        old = self.prefix / os.readlink(self.prefix / "current")
        (old / "old-release-only").write_text("remove with managed release")
        (self.home / ".quixi/providers.json").write_text("encrypted provider fixture")
        (self.home / ".quixi/credentials-master.json").write_text("wrapped key fixture")
        self.install("--reinstall")
        self.assertFalse((self.home / ".quixi" / "master-password-setup-required").exists())
        self.assertEqual((self.home / ".quixi/providers.json").read_text(), "encrypted provider fixture")
        self.assertEqual((self.home / ".quixi/credentials-master.json").read_text(), "wrapped key fixture")
        current = (self.prefix / "current").resolve()
        self.assertNotEqual(old, current)
        self.assertFalse(old.exists())
        self.assertTrue((current / ".tscdesk-release").is_file())
        self.assertEqual((self.home / ".quixi/data").read_text(), "keep me")
        self.assertEqual((self.home / ".gemini/mcp.json").read_text(), "keep MCP")

    def test_explicit_reinstall_stops_only_processes_from_managed_release(self):
        self.install()
        old = self.prefix / os.readlink(self.prefix / "current")
        sleeper = old / "bin/managed-sleeper"
        shutil.copy2(shutil.which("bash"), sleeper)
        process = subprocess.Popen([str(sleeper), "-c", "sleep 30 & wait"])
        try:
            self.assertIsNone(process.poll())
            self.install("--reinstall")
            process.wait(timeout=2)
            self.assertIsNotNone(process.returncode)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()

    def test_path_configuration_is_idempotent(self):
        first = self.install()
        second = self.install()
        line = 'export PATH="$HOME/.local/bin:$PATH"'
        for name in (".bashrc", ".profile"):
            self.assertEqual((self.home / name).read_text().splitlines().count(line), 1)
        self.assertIn("added ~/.local/bin", first.stdout)
        self.assertIn("already configured", second.stdout)
        self.assertIn("source", second.stdout)

    def test_login_shell_uses_profile_without_duplicating_path(self):
        env = self.env.copy()
        env["SHELL"] = "/bin/dash"
        self.install(env=env)
        self.install(env=env)
        line = 'export PATH="$HOME/.local/bin:$PATH"'
        self.assertEqual((self.home / ".profile").read_text().splitlines().count(line), 1)
        self.assertFalse((self.home / ".bashrc").exists())

    def test_foreign_launcher_conflict_before_install(self):
        self.launcher.parent.mkdir(parents=True)
        self.launcher.write_text("foreign launcher\n")
        result = self.install(ok=False)
        self.assertIn("conflict", result.stderr)
        self.assertFalse(self.prefix.exists())
        self.assertFalse(self.desktop.exists())
        self.assertEqual(self.launcher.read_text(), "foreign launcher\n")

    def test_foreign_desktop_conflict_and_wrong_install_marker(self):
        self.desktop.parent.mkdir(parents=True)
        self.desktop.write_text("[Desktop Entry]\nName=foreign\n")
        self.install(ok=False)
        self.assertFalse(self.prefix.exists())
        self.desktop.unlink()
        self.install()
        prior = self.launcher.read_bytes()
        self.prefix = self.home / "another-prefix"
        self.install(ok=False)
        self.assertFalse(self.prefix.exists())
        self.assertEqual(self.launcher.read_bytes(), prior)

    def test_foreign_prefix_and_symlinks_rejected(self):
        self.prefix.mkdir(parents=True)
        (self.prefix / "data").write_text("keep")
        self.install(ok=False)
        self.assertEqual((self.prefix / "data").read_text(), "keep")
        shutil.rmtree(self.prefix)
        outside = self.root / "outside"
        outside.mkdir()
        self.prefix.symlink_to(outside, target_is_directory=True)
        self.install(ok=False)
        self.assertEqual(list(outside.iterdir()), [])
        self.prefix.unlink()
        self.launcher.parent.mkdir(parents=True)
        self.launcher.symlink_to(outside / "missing")
        self.install(ok=False)
        self.assertFalse(self.prefix.exists())

    def test_parent_symlink_and_unmanaged_current_rejected(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.home / ".local").symlink_to(outside, target_is_directory=True)
        self.install(ok=False)
        self.assertEqual(list(outside.iterdir()), [])
        (self.home / ".local").unlink()
        self.install()
        (self.prefix / "current").unlink()
        (self.prefix / "current").symlink_to(outside)
        prior = self.launcher.read_bytes()
        self.install(ok=False)
        self.assertEqual(self.launcher.read_bytes(), prior)
        self.assertEqual(os.readlink(self.prefix / "current"), str(outside))

    def test_failed_update_preserves_previous_install(self):
        self.install()
        current = os.readlink(self.prefix / "current")
        prior = self.launcher.read_bytes(), self.desktop.read_bytes()
        (self.source / "resources/quixi-logo.svg").unlink()
        (self.source / "resources/quixi-mark.svg").unlink()
        self.install(ok=False)
        self.assertEqual(os.readlink(self.prefix / "current"), current)
        self.assertEqual((self.launcher.read_bytes(), self.desktop.read_bytes()), prior)
        (self.source / "target/release/TsCIDE").unlink()
        self.install(ok=False)
        self.assertEqual(os.readlink(self.prefix / "current"), current)
        self.launch(DISPLAY=":0", DBUS_SESSION_BUS_ADDRESS="fixture:existing")

    def test_commit_failure_rolls_back_managed_entries(self):
        self.install()
        current = os.readlink(self.prefix / "current")
        prior = self.launcher.read_bytes(), self.desktop.read_bytes()
        # Interpreter shim injects a filesystem failure at the commit point.
        # Production scripts have no test-only environment bypass or source override.
        self.command("python3", '''#!/usr/bin/python3
import sys
sys.argv = sys.argv[1:]  # Match real python3's argv when invoked with '-'.
code = sys.stdin.read().replace("os.replace(link, prefix / 'current')",
                               "(_ for _ in ()).throw(OSError('fixture commit failure'))")
exec(compile(code, '<installer fault fixture>', 'exec'))
''')
        result = self.install(ok=False)
        self.assertIn("fixture commit failure", result.stderr)
        self.assertEqual(os.readlink(self.prefix / "current"), current)
        self.assertEqual((self.launcher.read_bytes(), self.desktop.read_bytes()), prior)
        (self.bin / "python3").unlink()
        self.launch(DISPLAY=":0", DBUS_SESSION_BUS_ADDRESS="fixture:existing")

    def test_build_uses_stable_lockfile_and_requested_jobs(self):
        build_log = self.root / "build.json"
        toolchain_log = self.root / "toolchain.json"
        self.command("rustup", """#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
Path(os.environ['TOOLCHAIN_LOG']).write_text(json.dumps(sys.argv[1:]))
""")
        self.command("cargo", """#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
Path(os.environ['BUILD_LOG']).write_text(json.dumps(sys.argv[1:]))
""")
        env = self.env.copy()
        env["BUILD_LOG"] = str(build_log)
        env["TOOLCHAIN_LOG"] = str(toolchain_log)
        result = subprocess.run(["bash", str(self.source / "scripts/install-linux.sh"),
                                 "--skip-deps", "--prefix", str(self.prefix), "--jobs", "1", "--yes"],
                                cwd=self.root, env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(toolchain_log.read_text()), ["toolchain", "install", "stable", "--profile", "minimal"])
        self.assertEqual(json.loads(build_log.read_text()), [
            "+stable", "build", "--manifest-path", str(self.source / "Cargo.toml"),
            "--target-dir", str(self.source / "target"), "--locked", "--release", "--workspace", "--jobs", "1",
        ])

    def test_noninteractive_apt_needs_consent(self):
        result = subprocess.run(["bash", str(self.source / "scripts/install-linux.sh"),
                                 "--skip-build", "--prefix", str(self.prefix)],
                                cwd=self.root, env=self.env, capture_output=True, text=True,
                                stdin=subprocess.DEVNULL)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("consent required", result.stderr)
        self.assertFalse(self.prefix.exists())

    def test_forwarding_defaults_and_session_preservation(self):
        self.install()
        args = ["a space", "--flag", '$literal "quote"', "", "%percent"]
        self.launch(*args, DISPLAY=":42", WAYLAND_DISPLAY="wayland-1",
                    DBUS_SESSION_BUS_ADDRESS="fixture:existing", XDG_SESSION_TYPE="wayland")
        capture = json.loads(self.capture.read_text())
        self.assertEqual(capture["args"], args)
        env = capture["env"]
        self.assertEqual(env["QUIXI_WORKSPACE"], str(self.home))
        self.assertEqual(env["QUIXI_RESOURCES_DIR"], str((self.prefix / "current").resolve() / "resources"))
        for name, value in (("DISPLAY", ":42"), ("WAYLAND_DISPLAY", "wayland-1"),
                            ("DBUS_SESSION_BUS_ADDRESS", "fixture:existing"), ("XDG_SESSION_TYPE", "wayland")):
            self.assertEqual(env[name], value)
        self.assertNotIn("QUIXI_TEST_PROFILE", env)
        self.assertNotIn("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS", env)
        self.assertFalse(self.dbus_log.exists())
        self.assertFalse(self.keyring_log.exists(), "an available Secret Service must not be restarted")
        custom_resources = self.root / "custom resources"
        (custom_resources / "ui").mkdir(parents=True)
        (custom_resources / "ui/index.html").write_text("custom UI")
        self.launch(QUIXI_WORKSPACE="/custom workspace", QUIXI_RESOURCES_DIR=str(custom_resources),
                    DISPLAY=":0", DBUS_SESSION_BUS_ADDRESS="fixture:existing")
        env = json.loads(self.capture.read_text())["env"]
        self.assertEqual(env["QUIXI_WORKSPACE"], "/custom workspace")
        self.assertEqual(env["QUIXI_RESOURCES_DIR"], str(custom_resources))

    def test_test_profiles_keep_their_own_workspace(self):
        self.install()
        profile = str(self.root / "isolated profile")
        self.launch("--test-profile", profile, DISPLAY=":0", DBUS_SESSION_BUS_ADDRESS="fixture:existing")
        env = json.loads(self.capture.read_text())["env"]
        self.assertNotIn("QUIXI_WORKSPACE", env)
        self.launch(QUIXI_TEST_PROFILE=profile, DISPLAY=":0", DBUS_SESSION_BUS_ADDRESS="fixture:existing")
        env = json.loads(self.capture.read_text())["env"]
        self.assertEqual(env["QUIXI_TEST_PROFILE"], profile)
        self.assertNotIn("QUIXI_WORKSPACE", env)
        self.launch("--check", QUIXI_TEST_PROFILE=profile, DISPLAY=":0", DBUS_SESSION_BUS_ADDRESS="fixture:existing")
        self.launch(QUIXI_TEST_PROFILE=profile, QUIXI_WORKSPACE="/explicit workspace",
                    DISPLAY=":0", DBUS_SESSION_BUS_ADDRESS="fixture:existing")
        self.assertEqual(json.loads(self.capture.read_text())["env"]["QUIXI_WORKSPACE"], "/explicit workspace")

    def test_missing_resources_fail_before_app_start(self):
        self.install()
        result = self.launch("--check", QUIXI_RESOURCES_DIR=str(self.root / "missing resources"),
                             DISPLAY=":0", DBUS_SESSION_BUS_ADDRESS="fixture:existing", ok=False)
        self.assertIn("Missing UI resources", result.stderr)
        self.assertFalse(self.capture.exists())

    def test_graphical_failure_and_check(self):
        self.install()
        result = self.launch(ok=False)
        self.assertIn("No graphical session", result.stderr)
        self.assertFalse(self.capture.exists())
        self.launch("--help")
        self.launch("--check", DISPLAY=":0", DBUS_SESSION_BUS_ADDRESS="fixture:existing")
        self.assertFalse(self.capture.exists())
        self.assertFalse(self.dbus_log.exists())

    def test_existing_runtime_bus_reused_without_private_session(self):
        self.install()
        runtime = self.root / "runtime space"
        runtime.mkdir()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as bus:
            bus.bind(str(runtime / "bus"))
            self.launch(DISPLAY=":0", XDG_RUNTIME_DIR=str(runtime))
        address = json.loads(self.capture.read_text())["env"]["DBUS_SESSION_BUS_ADDRESS"]
        self.assertEqual(address, "unix:path=" + str(runtime / "bus").replace(" ", "%20"))
        self.assertFalse(self.dbus_log.exists())

    def test_missing_dbus_runs_once_and_secret_service_is_started_without_passwords(self):
        self.install()
        self.command("dbus-send", "#!/bin/sh\necho 'boolean false'\n")
        result = self.launch("arg", WAYLAND_DISPLAY="wayland-0")
        self.assertEqual(self.dbus_log.read_text().splitlines(), ["once"])
        self.assertEqual(json.loads(self.capture.read_text())["env"]["DBUS_SESSION_BUS_ADDRESS"],
                         "fixture:private")
        result = self.launch(DISPLAY=":0", DBUS_SESSION_BUS_ADDRESS="fixture:existing")
        self.assertIn("master-password vault", result.stderr)
        self.assertNotIn("Seahorse", result.stderr)
        self.assertEqual(self.dbus_log.read_text().splitlines(), ["once"])
        starts = self.keyring_log.read_text().splitlines()
        self.assertEqual(starts, ["--start --components=secrets", "--start --components=secrets"])
        self.assertNotIn("password", " ".join(starts).lower())

    def test_desktop_quoting_special_paths(self):
        self.install()
        lines = dict(line.split("=", 1) for line in self.desktop.read_text().splitlines() if "=" in line)
        self.assertEqual(lines["Name"], "TsCIDE")
        self.assertEqual(lines["StartupWMClass"], "TsCIDE")
        # freedesktop string unescape followed by Exec quote/backslash parsing.
        def unescape(value):
            escapes = {"s": " ", "n": "\n", "r": "\r", "t": "\t", "\\": "\\"}
            result, index = "", 0
            while index < len(value):
                if value[index] == "\\":
                    index += 1
                    result += escapes[value[index]]
                else:
                    result += value[index]
                index += 1
            return result
        command = shlex.split(unescape(lines["Exec"]))
        # shlex retains escapes of $ and `; Exec explicitly unescapes these too.
        command = [arg.replace('\\$', '$').replace('\\`', '`').replace('%%', '%') for arg in command]
        self.assertEqual(command, ["/usr/bin/env", "--", str(self.launcher)])
        self.assertEqual(lines["Terminal"], "false")
        self.assertEqual(unescape(lines["Icon"]), str(self.prefix / "current/resources/quixi-mark.svg"))

    def test_options_and_architecture(self):
        for args in (("--jobs", "0"), ("--jobs", "abc"), ("--jobs",), ("--prefix", "relative"), ("--nope",)):
            self.install(*args, ok=False)
            self.assertFalse(self.prefix.exists())
        env = self.env.copy()
        env["CARGO_BUILD_JOBS"] = "invalid"
        self.install(ok=False, env=env)
        self.install("--jobs", "3", env=env)
        self.command("uname", '#!/bin/sh\nif [ "$1" = -m ]; then echo riscv64; else echo Linux; fi\n')
        self.install(ok=False)

    def test_mark_icon_fallback_and_optional_helpers(self):
        (self.source / "resources/quixi-mark.svg").unlink()
        for helper in (self.source / "target/release").glob("quixi-*"):
            helper.unlink()
        self.install()
        self.assertIn("quixi-logo.svg", self.desktop.read_text())


if __name__ == "__main__":
    unittest.main()
