#!/usr/bin/env python3
"""Exercise the source installer in fixtures; never download, compile or install."""
import shlex
import unittest

from test_delivery import ROOT, Sandbox, shell_path


class SourceInstallerTests(Sandbox):
    def installer(self, *args):
        dest = self.directory / "bin"
        dest.mkdir(exist_ok=True)
        log = self.directory / "calls.log"
        source = (ROOT / "assets/install.sh").read_text(encoding="utf-8")
        source = source.replace('dest="$HOME/.local/bin"', 'dest=' + shlex.quote(shell_path(dest)))
        prefix = 'fixture_log=' + shlex.quote(shell_path(log)) + '\n' + r'''
# All downloads, compiler calls and archive extraction are isolated fixtures.
uname() { printf 'Darwin\n'; }
rustup() { printf 'rust-src\n'; }
cargo() { printf 'cargo %s\n' "$*" >> "$fixture_log"; }
curl() {
    output=
    while [ "$#" -gt 0 ]; do
        if [ "$1" = -o ]; then output=$2; shift 2; else url=$1; shift; fi
    done
    printf 'url %s\n' "$url" >> "$fixture_log"
    case "$url" in
        */releases/latest) printf '{"tag_name":"release/v1.2.4"}\n' > "$output" ;;
        *) : > "$output" ;;
    esac
}
tar() {
    while [ "$#" -gt 0 ]; do
        if [ "$1" = -C ]; then extracted=$2; shift 2; else shift; fi
    done
    mkdir -p "$extracted/target/release"
    printf '#!/bin/sh\necho fixture-ogedit\n' > "$extracted/target/release/ogedit"
    chmod +x "$extracted/target/release/ogedit"
}
'''
        result = self.run_shell(prefix + source, *args)
        self.assertEqual(result.returncode, 0, result.stderr)
        return dest, log.read_text(encoding="utf-8")

    def test_release_tag_namespace_and_fork_identity(self):
        dest, calls = self.installer()
        self.assertIn('Nucs/ogedit/archive/refs/tags/release/v1.2.4.tar.gz', calls)
        self.assertIn('cargo build --locked --release --config .cargo/release-nightly.toml', calls)
        self.assertIn('fixture-ogedit', (dest / 'ogedit').read_text())
        self.assertFalse((dest / 'edit').exists())
        self.assertFalse((dest / 'msedit').exists())

    def test_dev_preserves_unrelated_alias(self):
        dest = self.directory / "bin"
        dest.mkdir()
        (dest / 'ogmsedit').write_text('administrator alias', encoding='utf-8')
        _, calls = self.installer('--dev')
        self.assertIn('Nucs/ogedit/archive/refs/heads/main.tar.gz', calls)
        self.assertNotIn('/releases/latest', calls)
        self.assertEqual((dest / 'ogmsedit').read_text(), 'administrator alias')

    def test_preserves_dangling_unrelated_alias(self):
        dest = self.directory / 'bin'
        dest.mkdir()
        alias = dest / 'ogmsedit'
        try:
            alias.symlink_to('administrator-missing-target')
        except OSError as error:
            self.skipTest(f'Host cannot create a symbolic link: {error}')
        self.installer('--dev')
        self.assertTrue(alias.is_symlink())
        self.assertEqual(str(alias.readlink()), 'administrator-missing-target')


if __name__ == '__main__':
    unittest.main(verbosity=2)
