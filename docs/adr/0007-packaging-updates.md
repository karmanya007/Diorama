# ADR-007: Windows packaging and updates

Status: accepted (2026-10-28)

## Context

Phase 6 ships Lumen to people who do not build it: an installer, a way to
update, and the daemon, web host, CLI and DXC placed where lumen-ui and
lumend look for them (beside each other, `dxc\dxc.exe` beside lumend).
No code-signing certificate yet.

## Decision

- **Tauri's NSIS bundler**, per-user install (no admin) into
  `%LOCALAPPDATA%\Lumen`. lumend, lumen-webhost and `lumen` are Tauri
  `externalBin`s; DXC is a resource in `dxc\`.
- **Only `dxc.exe` + `dxcompiler.dll`** of the pinned DXC release ship, with
  their LLVM and MIT licences. `dxil.dll` (closed, Microsoft licence with
  distributor obligations) is left out: Lumen compiles to SPIR-V, which does
  not need it.
- **Bundle settings live in `tauri.bundle.conf.json`**, merged only by
  `tauri build` (`scripts/bundle.ps1`). Tauri's build script checks that
  bundled files exist, so in `tauri.conf.json` they would break `cargo build`,
  clippy and CI for everyone.
- **Updates: tauri-plugin-updater** reading `latest.json` from the latest
  GitHub Release, installers signed with a minisign key (public half in
  `tauri.conf.json`, private half only with the maintainer and in the
  `TAURI_SIGNING_PRIVATE_KEY` secret). Checked only when the user asks
  (Settings → Updates); installs passively and relaunches. The plugin runs
  without its `zip` feature (Windows updates are the installer itself; the
  feature brought `zip` 4 with open advisories) and with the OS's TLS
  (`native-tls`: SChannel, Windows certificate store).
- **Installer hooks** (`installer-hooks.nsh`): before installing over or
  removing Lumen, quit lumend (`lumen quit`) and stop its processes started
  from the install folder; on uninstall, remove the sign-in entry, and with
  "delete application data" ticked, Lumen's settings and caches.
- **Unsigned** for now: SmartScreen warns on first run. Signing can be added
  to the release workflow later without changing the rest.
- **Releases**: pushing a `v*` tag equal to the workspace version runs
  `.github/workflows/release.yml`, which builds, signs the update file,
  writes `latest.json` and publishes the GitHub Release.

## Consequences

- The updater's download URLs must be reachable without credentials: the
  repository (or wherever `latest.json` and the installers are hosted) has
  to be public before the first release that installed copies should see.
- Linux/macOS packaging (Phases 7/8) will need the plugin's archive support
  back (tar for AppImage/app bundles).
- WebView2 comes from Windows (Tauri's default bootstrapper download if
  missing).
