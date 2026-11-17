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

The binary answers `--version` (prints `inlet <version>`) before it opens a window, and the installer's `--veloapp-*` hook arguments through the Velopack call in `crates/inlet-app/src/main.rs` (`inlet::update::startup`).

### Updates

Inlet updates itself from its GitHub releases (`crates/inlet-app/src/update.rs`, the Updates section of the Desktop settings tab). Every release carries, next to the downloads, the update feed of each system: the full package (`no.opland.inlet-<version>-beta-<os>-full.nupkg`) and the feed file `releases.beta-<os>.json` that lists it with its size and SHA-256. `release.yml` collects them from the `velopack/` folder that `build-packages.sh` writes. The channel is `beta-win` or `beta-linux` (`beta-osx` for macOS): one feed file per system, so the systems do not overwrite each other's files in the same release. The channel is fixed when a package is built (`--channel`), the app updates only on its own channel, and it never goes back to an older version. The release notes in the feed are the version's section of `CHANGELOG.md` (`velopack/release-notes.sh`); a build between releases has none.

No delta packages are built: each release is packed on its own, so an update is the full package (about 20 to 30 MB).

What the app does with it: checks at most every 6 hours (also a few minutes after start), downloads a newer version in the background, checks its size and SHA-256 against the feed and against the digest GitHub computed for the asset, and installs it the next time Inlet starts, or when the user picks "Restart to update". The switch in the settings turns the checks, the download and the install at start off. The app only talks to `api.github.com` and the download addresses of this repository, over HTTPS. A copy that was not installed by the installer (a tarball, a development build) cannot update itself.

`velopack/update-test.sh` tests the whole flow against a fake release in a local folder: it packs a probe program as the app in two versions, finds, downloads and applies the update with the real update code, and checks that a downgrade, a package with a wrong hash, a feed without SHA-256, another package id and another channel are all refused. It runs on a Linux machine with a Rust toolchain and needs a .NET runtime (it installs one into a scratch folder when there is none), so run it in the build container: `scripts/docker-build.sh --exec packaging/velopack/update-test.sh`. CI does not run it.

### Running the AppImage

The AppImage uses the standard AppImage runtime, which needs FUSE 2. Ubuntu 24.04 does not install it by default: run `sudo apt install libfuse2t64` (older releases: `libfuse2`), or start the image with `./inlet-<version>-linux-x86_64.AppImage --appimage-extract-and-run`, which needs no FUSE. The CI smoke test uses the second form.

### Deep links and login item

The app registers `inlet://` and its login item itself when it starts (`crates/inlet-platform/src/deep_link.rs`, `autostart.rs`). Inside an AppImage the running executable is in a temporary mount, so both register the `.AppImage` file instead, taken from the `APPIMAGE` variable the AppImage runtime sets (`launch_path.rs`). The desktop entry also declares the scheme so that tools that integrate AppImages register it.

On Windows the uninstaller removes the install folder, the shortcut and the uninstall entry. The keys the app wrote itself into the user's registry (the `inlet://` handler, the login item and the notification identity) are removed by the uninstall hook (`inlet_platform::uninstall::cleanup`, run through Velopack's `--veloapp-uninstall` argument). An AppImage has no uninstaller; the desktop entries the app wrote stay until the file is deleted by hand.

### Taskbar identity

The Start menu shortcut carries the AppUserModelID `no.opland.inlet` (`--aumid`), the same id the running app sets, so notifications and taskbar grouping use one identity.
