"""Root ChromeOS entry point: fixtures only, no packages, Rust, GUI or GPU."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import unittest

REPO = Path(__file__).resolve().parents[2]


@unittest.skipUnless(platform.system() == "Linux", "requires Linux bash")
class ChromeOSStartTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="tscdesk-root-launch-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "source with spaces"
        self.home = Path(self.temp.name) / "home"
        (self.root / "scripts").mkdir(parents=True)
        self.home.mkdir()
        shutil.copyfile(REPO / "iniciar-chromeos.sh", self.root / "iniciar-chromeos.sh")
        self.env = dict(os.environ, HOME=str(self.home), DISPLAY=":fixture")
        for key in ("WAYLAND_DISPLAY", "QUIXI_TEST_PROFILE", "QUIXI_WORKSPACE"):
            self.env.pop(key, None)
        self.log = self.home / "installer.json"
        self.env["INSTALL_LOG"] = str(self.log)
        self.env["CAPTURE"] = str(self.home / "launcher.json")
        # Fixture installer produces the same managed layout without touching the system.
        (self.root / "scripts/install-linux.sh").write_text('''#!/usr/bin/env bash
set -euo pipefail
python3 - "$@" <<'PY'
import hashlib, json, os, pathlib, sys
pathlib.Path(os.environ['INSTALL_LOG']).write_text(json.dumps(sys.argv[1:]))
home = pathlib.Path(os.environ['HOME'])
prefix = pathlib.Path(os.path.abspath(home / '.local/opt/tscdesk'))
release = prefix / 'releases/fixture'
release.mkdir(parents=True)
(release / 'bin').mkdir()
(release / 'bin/launch-linux.sh').write_text('#!/bin/sh\\nexit 0\\n')
mark = 'TSCDesk Linux installer managed v1\\n'
(prefix / '.tscdesk-managed').write_text(mark)
(release / '.tscdesk-release').write_text(mark)
(prefix / 'current').symlink_to('releases/fixture')
launcher = home / '.local/bin/tscdesk'
launcher.parent.mkdir(parents=True)
identity = hashlib.sha256(str(prefix).encode()).hexdigest()
launcher.write_text('#!/usr/bin/env python3\\n# TSCDesk-managed-v1 ' + identity + '\\nimport json,os,sys\\nfrom pathlib import Path\\nPath(os.environ["CAPTURE"]).write_text(json.dumps(sys.argv[1:]))\\n')
launcher.chmod(0o755)
PY
''')

    def run_script(self, *args, env=None):
        return subprocess.run(["bash", str(self.root / "iniciar-chromeos.sh"), *args],
                              cwd=self.home, env=env or self.env, text=True, capture_output=True)

    def user_only(self):
        if os.geteuid() == 0:
            self.skipTest("normal desktop user required")

    def test_help_and_bash_syntax(self):
        subprocess.run(["bash", "-n", str(self.root / "iniciar-chromeos.sh")], check=True)
        result = self.run_script("--help")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Uso:", result.stdout)
        self.assertFalse(self.log.exists())

    def test_installs_once_and_forwards_arguments(self):
        self.user_only()
        result = self.run_script("--yes", "--skip-build", "--", "--test-profile", "/profile with spaces")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.log.read_text())[-2:], ["--yes", "--skip-build"])
        self.assertIn("1", json.loads(self.log.read_text()))
        capture = Path(self.env["CAPTURE"])
        self.assertEqual(json.loads(capture.read_text()), ["--test-profile", "/profile with spaces"])
        self.log.unlink()
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.log.exists(), "ordinary launches must not reinstall or warm up")
        self.assertEqual(json.loads(capture.read_text()), [])
        result = self.run_script("--check")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(capture.read_text()), ["--check"])

    def test_diagnostic_does_not_install(self):
        self.user_only()
        result = self.run_script("--check")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.log.exists())

    def test_no_graphical_session_does_not_install(self):
        self.user_only()
        env = dict(self.env)
        env.pop("DISPLAY")
        result = self.run_script(env=env)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.log.exists())

    def test_foreign_launcher_is_never_executed(self):
        self.user_only()
        launcher = self.home / ".local/bin/tscdesk"
        launcher.parent.mkdir(parents=True)
        launcher.write_text('#!/bin/sh\ntouch "$HOME/should-not-exist"\n')
        launcher.chmod(0o755)
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.home / "should-not-exist").exists())
        self.assertFalse(self.log.exists())

    def test_application_help_is_forwarded_and_home_is_normalized(self):
        self.user_only()
        env = dict(self.env, HOME=str(self.home / '..' / self.home.name))
        result = self.run_script("--", "--help", env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(Path(self.env["CAPTURE"]).read_text()), ["--help"])

    def test_release_symlinks_are_rejected_before_launch(self):
        self.user_only()
        self.assertEqual(self.run_script().returncode, 0)
        capture = Path(self.env["CAPTURE"])
        capture.unlink()
        release = self.home / '.local/opt/tscdesk/releases/fixture'
        marker = release / '.tscdesk-release'
        external = self.home / 'foreign-marker'
        marker.rename(external)
        marker.symlink_to(external)
        self.assertNotEqual(self.run_script("--check").returncode, 0)
        self.assertFalse(capture.exists())
        marker.unlink()
        external.rename(marker)
        script = release / 'bin/launch-linux.sh'
        script.unlink()
        script.symlink_to('/bin/bash')
        self.assertNotEqual(self.run_script("--check").returncode, 0)
        self.assertFalse(capture.exists())

    def test_root_rejected(self):
        if os.geteuid() != 0:
            self.skipTest("root-only rejection test")
        result = self.run_script()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("root", result.stderr)
        self.assertFalse(self.log.exists())
