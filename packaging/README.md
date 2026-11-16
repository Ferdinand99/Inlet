# Packaging

How the release files are built. The macOS bundle has its own scripts in `macos/`; the notes below cover Windows and Linux.

## Packages

Both are built with [Velopack](https://velopack.io) from the release-profile binary. CI builds them in the `windows-release` and `linux-release` jobs of `.github/workflows/build.yml`: on pushes to `v0`, on `main`, on tags (through `release.yml`) and on manual runs with `release` ticked. Pull requests do not build them. The version is the one in `[workspace.package]` of `Cargo.toml`. Nothing is code-signed yet.

| File | What it is |
|---|---|
| `inlet-<version>-windows-x86_64-setup.exe` | Installer. Installs for the current user into `%LocalAppData%\no.opland.inlet`, needs no administrator rights, adds a Start menu shortcut and an entry in "Apps & features". |
| `inlet-<version>-windows-x86_64.zip` | Portable copy. |
| `inlet-<version>-linux-x86_64.AppImage` | AppImage (Velopack's Linux package). Make it executable and run it. |
| `inlet-<version>-linux-x86_64.tar.gz` | The binary, `LICENSE` and `THIRD-PARTY-NOTICES`. |

`release.yml` copies these four files into the release and lists them in `SHA256SUMS.txt`; each pattern must match exactly one file or the release stops.

### Where things are

| Path | What it is |
|---|---|
| `velopack/vpk.sh` | Runs Velopack's packer (`vpk`) at a pinned version. It downloads the package from the Velopack release, checks its SHA-256 and unpacks it; nothing else is fetched. To update it, change `VPK_VERSION` and `VPK_SHA256` together. |
| `windows/build-packages.sh` | Builds the Setup and the zip. The icon is `assets/brand/windows/inlet.ico`. |
| `windows/smoke-test.ps1` | Installs the Setup silently, checks the Start menu shortcut and the registry entry, runs the installed `inlet.exe --version`, uninstalls and checks that everything is gone. |
| `linux/build-packages.sh` | Assembles the AppDir from the files in `linux/` and the icons in `assets/brand/linux`, packs it with `vpk`, and makes the tarball. |
| `linux/no.opland.inlet.desktop` | Desktop entry. Its name is the app id (`no.opland.inlet`), the same as the window's app id and the icon names, so desktops match the window to the entry. It declares `x-scheme-handler/inlet`. |
| `linux/AppRun` | The AppImage's entry point. |
| `linux/smoke-test.sh` | Runs the AppImage under Xvfb (`--version`) and checks what is inside it. |

The binary answers `--version` (prints `inlet <version>`) and the installer's `--veloapp-*` hook arguments (exits at once) before it opens a window; see `crates/inlet-app/src/main.rs`.

### Updates

The packages are laid out for Velopack auto-update, but nothing in the app checks for updates yet. `build-packages.sh` also writes a `velopack/` folder next to the packages in the build artifact (`inlet-windows-x86_64-packages`, `inlet-linux-x86_64`). It holds the full package (`*-full.nupkg`) and the release feed (`releases.<channel>.json`). Publishing an update feed means keeping those files with each release and passing the previous ones to `vpk` as input; the package layout does not change.

### Deep links

The app registers `inlet://` itself when it starts (`crates/inlet-platform/src/deep_link.rs`). The desktop entry also declares the scheme so that tools that integrate AppImages register it.
