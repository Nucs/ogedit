#!/usr/bin/env python3
"""Delivery regressions in disposable files/repos; never install or publish.

Run: python tools/test_delivery.py
Uses Git Bash on Windows and bash elsewhere. No administrator rights required.
"""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

sys.dont_write_bytecode = True
from release_metadata import metadata

ROOT = Path(__file__).resolve().parents[1]
BASH = (str(Path(os.environ.get("ProgramFiles", "C:/Program Files")) / "Git/bin/bash.exe")
        if os.name == "nt" else shutil.which("bash"))


def shell_path(path):
    value = str(Path(path).resolve()).replace("\\", "/")
    return f"/{value[0].lower()}{value[2:]}" if os.name == "nt" else value


class Sandbox(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ogedit-delivery-")
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.profile = self.directory / "ogedit-editor.sh"

    def run_shell(self, source, *args, env=None, cwd=None):
        script = self.directory / "fixture.sh"
        script.write_text(source, encoding="utf-8", newline="\n")
        return subprocess.run([BASH, shell_path(script), *args], cwd=cwd,
                              env={**os.environ, **(env or {})},
                              capture_output=True, text=True, encoding="utf-8")

    def fixture_source(self, relative):
        source = (ROOT / relative).read_text(encoding="utf-8")
        # Redirect every system write in copied maintainer scripts to this sandbox.
        source = source.replace('/etc/profile.d/ogedit-editor.sh', shell_path(self.profile))
        confmodule = self.directory / "debconf.sh"
        confmodule.write_text('db_get() { RET="${TEST_SET_EDITOR:-false}"; }\ndb_purge() { :; }\n', encoding="utf-8")
        source = source.replace('/usr/share/debconf/confmodule', shell_path(confmodule))
        return source


class VersionTests(Sandbox):
    def version(self, value, tag=None):
        manifest = self.directory / "Cargo.toml"
        manifest.write_text('[package]\nversion = "' + value + '"\n', encoding="utf-8")
        return metadata(manifest, tag)

    def test_stable_and_arbitrary_prerelease(self):
        self.assertEqual(self.version("1.2.4", "release/v1.2.4")["IS_PRERELEASE"], "false")
        value = self.version("1.3.0-preview.2+build.9")
        self.assertEqual(value["IS_PRERELEASE"], "true")
        self.assertEqual(value["VERSION_CLEAN"], "1.3.0")
        self.assertEqual(value["DEBIAN_VERSION"], "1.3.0~preview.2+build.9")
        self.assertEqual(value["RPM_RELEASE"], "0.preview.2")
        self.assertEqual(self.version("1.3.0")["RPM_RELEASE"], "1")
        self.assertEqual(self.version("1.3.0+build-id")["DEBIAN_VERSION"], "1.3.0+build-id")

    def test_tag_mismatch_and_invalid_versions_fail(self):
        with self.assertRaises(ValueError):
            self.version("1.2.4", "release/v1.2.3")
        for value in ("01.2.3", "1.2.3-beta.01", "1.2.3;echo-bad", "256.0.0", "1.256.0", "1.2.65536"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.version(value)


class ProfileTests(Sandbox):
    DEB_CONTENT = '# Set EDITOR to ogedit (installed by ogedit package)\nexport EDITOR="/usr/bin/ogedit"\n'

    def test_deb_remove_cleans_managed_profile(self):
        self.profile.write_text(self.DEB_CONTENT, encoding="utf-8", newline="\n")
        result = self.run_shell(self.fixture_source("deb/postrm"), "remove")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.profile.exists())

    def test_deb_upgrade_preserves_profile(self):
        self.profile.write_text(self.DEB_CONTENT, encoding="utf-8", newline="\n")
        result = self.run_shell(self.fixture_source("deb/postrm"), "upgrade")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.profile.read_text(), self.DEB_CONTENT)

    def test_deb_remove_and_purge_preserve_admin_replacement(self):
        for operation in ("remove", "purge"):
            self.profile.write_text('export EDITOR="vim"\n', encoding="utf-8")
            result = self.run_shell(self.fixture_source("deb/postrm"), operation)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.profile.read_text(), 'export EDITOR="vim"\n')

    def test_deb_configure_enable_then_disable(self):
        source = self.fixture_source("deb/postinst")
        result = self.run_shell(source, "configure", env={"TEST_SET_EDITOR": "true"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.profile.read_text(), self.DEB_CONTENT)
        result = self.run_shell(source, "configure", env={"TEST_SET_EDITOR": "false"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.profile.exists())

    def test_deb_configure_preserves_admin_profile(self):
        self.profile.write_text("# administrator settings\n", encoding="utf-8")
        for choice in ("true", "false"):
            result = self.run_shell(self.fixture_source("deb/postinst"), "configure", env={"TEST_SET_EDITOR": choice})
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(self.profile.read_text(), "# administrator settings\n")

    def test_rpm_enable_disable_and_unset_editor_status(self):
        # Only the privilege probe is stubbed; every write is already redirected.
        source = 'id() { printf "0\\n"; }\n' + self.fixture_source("rpm/ogedit-set-editor")
        result = self.run_shell(source, "--enable")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('export EDITOR="/usr/bin/ogedit"', self.profile.read_text())
        result = self.run_shell(source, "--disable")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.profile.exists())
        result = self.run_shell('unset EDITOR\n' + source, "--status")
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_rpm_preserves_admin_profile(self):
        source = 'id() { printf "0\\n"; }\n' + self.fixture_source("rpm/ogedit-set-editor")
        self.profile.write_text("# administrator settings\n", encoding="utf-8")
        self.assertNotEqual(self.run_shell(source, "--enable").returncode, 0)
        self.assertEqual(self.run_shell(source, "--disable").returncode, 0)
        self.assertEqual(self.profile.read_text(), "# administrator settings\n")
        self.assertIn("administrator-managed", self.run_shell(source, "--status").stdout)

    def test_symlink_profiles_are_not_followed(self):
        target = self.directory / "unrelated.txt"
        target.write_text("do not change\n", encoding="utf-8")
        try:
            self.profile.symlink_to(target)
        except OSError as error:
            self.skipTest(f"Symlinks unavailable: {error}")
        result = self.run_shell(self.fixture_source("deb/postinst"), "configure", env={"TEST_SET_EDITOR": "true"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.run_shell(self.fixture_source("deb/postrm"), "remove").returncode, 0)
        self.assertTrue(self.profile.is_symlink())
        self.assertEqual(target.read_text(), "do not change\n")

    def test_macos_user_profile_body_is_idempotent(self):
        source = (ROOT / "pkg/postinstall-editor-env").read_text(encoding="utf-8")
        self.assertIn('/usr/bin/sudo -u "$CONSOLE_USER" -H -- /bin/sh -s', source)
        self.assertNotIn("eval ", source)
        self.assertNotIn("chown ", source)
        body = source.split("<<'OGEDIT_USER_SCRIPT'\n", 1)[1].rsplit("OGEDIT_USER_SCRIPT", 1)[0]
        home = self.directory / "user home"
        home.mkdir()
        rc = home / ".zshrc"
        rc.write_text("# existing user settings\n", encoding="utf-8")
        for _ in range(2):
            result = self.run_shell(body, shell_path(home))
            self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(rc.read_text().count('export EDITOR="/usr/local/bin/ogedit"'), 1)
        self.assertTrue(rc.read_text().startswith("# existing user settings\n"))


class ReleaseTagTests(Sandbox):
    def git(self, *args, cwd=None):
        return subprocess.check_output(["git", *args], cwd=cwd or self.repo, stderr=subprocess.PIPE, text=True).strip()

    def setUp(self):
        super().setUp()
        self.repo = self.directory / "working"
        self.remote = self.directory / "remote.git"
        self.git("init", "--bare", str(self.remote), cwd=self.directory)
        self.git("init", str(self.repo), cwd=self.directory)
        self.git("config", "user.name", "Delivery Test")
        self.git("config", "user.email", "delivery@example.invalid")
        (self.repo / "fixture").write_text("one\n", encoding="utf-8")
        self.git("add", "fixture")
        self.git("commit", "-m", "fixture")
        self.commit = self.git("rev-parse", "HEAD")
        self.git("remote", "add", "origin", str(self.remote))
        self.output = self.directory / "output"

    def tag(self):
        return self.run_shell((ROOT / "tools/create_release_tag.sh").read_text(), cwd=self.repo,
                              env={"TAG": "release/v1.2.4", "RELEASE_COMMIT": self.commit,
                                   "GITHUB_OUTPUT": shell_path(self.output)})

    def test_new_tag_and_same_commit_retry_publish(self):
        result = self.tag()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.output.read_text(), "publish=true\n")
        self.output.write_text("")
        self.assertEqual(self.tag().returncode, 0)
        self.assertEqual(self.output.read_text(), "publish=true\n")

    def test_existing_version_at_older_commit_skips(self):
        self.git("tag", "release/v1.2.4")
        (self.repo / "fixture").write_text("new unrelated change\n")
        self.git("commit", "-am", "unrelated change")
        self.commit = self.git("rev-parse", "HEAD")
        self.assertEqual(self.tag().returncode, 0)
        self.assertEqual(self.output.read_text(), "publish=false\n")

    def test_identical_remote_race_is_accepted(self):
        self.git("tag", "release/v1.2.4")
        self.git("push", "origin", "refs/tags/release/v1.2.4")
        self.git("tag", "-d", "release/v1.2.4")
        result = self.tag()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.output.read_text(), "publish=true\n")

    def test_conflicting_remote_tag_fails(self):
        self.git("tag", "release/v1.2.4")
        self.git("push", "origin", "refs/tags/release/v1.2.4")
        self.git("tag", "-d", "release/v1.2.4")
        (self.repo / "fixture").write_text("two\n")
        self.git("commit", "-am", "second fixture")
        self.commit = self.git("rev-parse", "HEAD")
        self.assertNotEqual(self.tag().returncode, 0)
        self.assertFalse(self.output.exists())

    def test_missing_remote_fails_instead_of_claiming_race(self):
        self.git("remote", "set-url", "origin", str(self.directory / "missing.git"))
        self.assertNotEqual(self.tag().returncode, 0)
        self.assertFalse(self.output.exists())


class SyntaxTests(unittest.TestCase):
    def test_shell_syntax(self):
        for relative in ("deb/config", "deb/postinst", "deb/postrm", "pkg/postinstall-editor-env",
                         "rpm/ogedit-set-editor", "rpm/ogedit-editor.sh.sample", "tools/create_release_tag.sh"):
            with self.subTest(path=relative):
                subprocess.run([BASH, "-n", shell_path(ROOT / relative)], check=True, capture_output=True)

    def test_xml_and_quoted_windows_editor(self):
        for relative in ("pkg/Distribution.xml.template", "wix/main.wxs"):
            tree = ET.parse(ROOT / relative)
            if relative.startswith("wix"):
                variables = tree.findall('.//{http://wixtoolset.org/schemas/v4/wxs}Environment')
                editor = next(item for item in variables if item.get("Name") == "EDITOR")
                self.assertEqual(editor.get("Value"), '"[INSTALLFOLDER]ogedit.exe"')


if __name__ == "__main__":
    unittest.main(verbosity=2)
