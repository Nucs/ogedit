# Legacy delivery review

Reviewed 2026-10-17 against old OGEdit `01cf53f`, with repairs applied to the current working tree. Commit provenance is in [the complete history map](legacy-commit-map.md). These checks exercise packaging code in disposable directories; they do not install packages or publish releases.

## Confirmed defects repaired

| Area | Historical failure | Current behavior |
| --- | --- | --- |
| Automatic releases | A tag pushed by `GITHUB_TOKEN` does not trigger the separate release-on-push workflow. The documented automatic path could create a tag without a release. | Auto-tag calls `release.yml` as a reusable workflow directly. Human tag pushes remain supported. No new token or secret is needed. |
| Tag races and failures | Every failed `git push` was described as a harmless concurrent tag. Authentication, network and conflicting-tag failures could appear successful. | A failed push succeeds only after fetching the remote tag and proving its peeled commit equals the intended commit. Other errors fail the job. |
| Retry after build failure | An existing tag could cause a rerun to skip the only release invocation. | A tag already pointing to the same commit continues/retries its release. A tag for an older version commit skips publication. |
| Release identity | Cargo version was used independently of the triggering tag; build jobs had repository write permission and a moving nightly install. | Tag, Cargo version and expected commit are checked before building. All jobs use the pinned commit; builds are locked and use the repository toolchain. Only publication/tag creation gets write permission. |
| Prerelease handling | Only alpha/beta/rc strings counted as prereleases; Debian/RPM stripped prerelease information entirely. | Semantic-version validation recognizes arbitrary valid prereleases. Debian uses `~` ordering and RPM uses a `0.<prerelease>` release so final packages supersede previews. Windows/macOS retain their required numeric core version. |
| macOS ZIP checksum | The shared Unix ZIP step called `sha256sum`, which is not a standard macOS utility. | Uses `shasum -a 256`, also available on the Linux build runner. |
| macOS optional EDITOR | The root postinstall script appended to and changed ownership of a user-selected shell RC, following user-controlled symlinks. It also used `eval` to resolve the home directory. | Home lookup uses Directory Services. All profile writes run with the console user's identity; there is no root append/chown or eval. Headless/root console sessions receive instructions without editing root's profile. |
| Debian uninstall | Ordinary removal left the optional profile script pointing at a removed `/usr/bin/ogedit`; only purge cleaned it. | Remove/disappear/purge clean the exact managed profile. Upgrades preserve it. Administrator replacements and symlinks are preserved. Enable uses a temporary file and atomic rename. |
| RPM uninstall and opt-in | The helper created a profile file outside the RPM manifest, with no uninstall cleanup; enable/disable could overwrite/remove administrator changes. | Final uninstall calls the helper before removing it. Only the exact managed profile is removed/replaced; administrator files and symlinks are preserved. Status reports administrator ownership correctly. |
| Windows EDITOR | MSI stored a full executable path containing `Program Files` without quoting for shell consumers. | The environment value quotes the executable path. |
| AppImage tool execution | A mutable continuous tool download executed without integrity validation, and a multi-image ICO conversion could emit numbered PNGs instead of the required icon. | Builder and runtime use versioned official URLs and checked SHA256 digests. Existing repository SVG is copied as the AppDir icon. No image conversion or base64 fallback is needed. |

The GitHub behavior and reusable-workflow rules were verified against [GitHub's event documentation](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows) and [reusable workflow documentation](https://docs.github.com/en/actions/how-tos/reuse-automations/reuse-workflows). SVG support and icon naming follow the [AppDir specification](https://docs.appimage.org/reference/appdir.html). The [official appimagetool documentation](https://github.com/AppImage/appimagetool) documents the explicit runtime override used to prevent another moving runtime download.

## Local validation

- `python tools/test_delivery.py`: **18 tests pass**, covering real Git operations against disposable local bare repositories, tag retry/race/error handling, version/tag validation, Debian enable/remove/upgrade behavior, administrator-file preservation, symlink preservation, RPM enable/disable/status, macOS user-profile idempotency, shell syntax and installer XML.
- No symlink test was skipped on this host. Shell tests redirect system paths to their temporary directory and stub only the privilege/debconf probes; no installer ran against the machine.
- Actionlint **1.7.12** validates both changed workflows. Its downloaded Windows archive was verified against the official GitHub release digest. Embedded shell code was additionally checked with Bash syntax parsing; actionlint's optional external ShellCheck/Pyflakes integrations were not installed.
- WiX **5.0.2** with its matching UI extension successfully built `target/legacy-delivery-tools/ogedit-windows-x64.msi` from the existing release executable. WiX tools/extensions were installed only under the ignored target directory. This validates installer compilation and the x64 package structure, not its install/uninstall UI or environment effects.
- `git diff --check` passed for the changed delivery sources.
- Downloaded official AppImage assets matched both pinned digests before any execution; the Linux executables were **not executed** on this Windows host.

| Artifact | Pinned source | SHA256 |
| --- | --- | --- |
| appimagetool x86_64 | AppImage/appimagetool 1.9.1 | `ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0` |
| AppImage runtime x86_64 | AppImage/type2-runtime 20251108 | `2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d` |

Local logs: `target/qa-legacy-delivery-tests.log`, `target/qa-legacy-msi-build.log`. Delivery regressions also run in the release workflow's verification job before matrix builds.

## Validation limits

No remote workflow was dispatched and no tag/release was pushed. A real GitHub release run still needs to exercise runner tools, permissions and all package uploads together. The sandbox Git tests prove the tag decision/error paths; they do not simulate GitHub's service.

Windows MSI installation/upgrade/uninstall and optional EDITOR behavior were not executed. ARM64 MSI, macOS PKG, Linux DEB/RPM and AppImage builds and native installation were not executed on this Windows host. macOS console-user discovery and privilege dropping were source-reviewed; only its unprivileged profile-writing body was executed in a fixture. No desktop automation was used.

Package compatibility across Linux distributions and macOS versions is not established by Rust cross-target checks or browser TUI QA. Native package managers may need additional dependency/platform validation before publication. Windows/macOS installer version ordering uses the numeric core, so preview and final packages with the same core require explicit native upgrade testing. No claim of universal platform compatibility or absolute failure-proof behavior is made.
