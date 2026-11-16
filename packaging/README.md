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

### The web view helper

Flows that need a real web page (a sign-in at an identity provider, a payment, an OAuth consent) show it in a separate helper program, `inlet-web` (`crates/inlet-web`). Inlet starts it when it is needed and it ends with its window. It ships next to `inlet` in every package, and Inlet looks for it there (`crates/inlet-web/src/client.rs`):

| Package | Where the helper is |
|---|---|
| Windows Setup and zip | `inlet-web.exe` next to `inlet.exe` |
| Linux AppImage | `usr/bin/inlet-web` next to `usr/bin/inlet` |
| Linux tar.gz | `inlet-web` next to `inlet` |
| macOS `Inlet.app` | `Contents/MacOS/inlet-web` |
| macOS zip | `inlet-web` next to `inlet` |

The release jobs build both with `cargo build --release --locked -p inlet -p inlet-web`, and the packaging scripts refuse to build without the helper next to the binary. The helper uses the system's web engine and adds little to the download; the sizes of the files are in the job summary of each release build.

What the system must have:

- Windows: the WebView2 runtime. It comes with Windows 10 and 11 (Evergreen). When it is missing the helper reports `runtime_missing`, and Inlet can say so.
- macOS: nothing (WKWebView is part of the system).
- Linux: WebKitGTK 4.1 (`libwebkit2gtk-4.1-0`, from the distribution) and GTK 3. The helper links them dynamically and they are not in the AppImage. Without them the helper cannot start and the flow fails with `helper_exited`. The development files are in the builder image and the CI jobs that build the helper.

`packaging/macos` and the signing workflows are the same as on main (a check enforces it), so the macOS bundle gets the helper in `build.yml` (the step "Assemble Inlet.app"), after `build-app.sh`. **The signing workflow must be changed on main before a signed release can include the helper.** `macos-sign.yml` signs `Inlet.app` without `--deep`, so a second executable in `Contents/MacOS` stays unsigned and notarisation rejects the app. The helper must be signed first, with the hardened runtime and a timestamp, then the bundle (signing the bundle seals the helper's signature):

```sh
codesign --force --options runtime --timestamp   --keychain "$KEYCHAIN" --sign "$SIGN_IDENTITY"   build/Inlet.app/Contents/MacOS/inlet-web
```

as a step before "Sign the app". The helper needs no entitlements (WKWebView runs in Apple's own web content process), so no entitlements file is passed. `build-app.sh` should also copy the helper (and check its `arm64` slice), so that the step in `build.yml` can go.

### Updates

The packages are laid out for Velopack auto-update, but nothing in the app checks for updates yet. `build-packages.sh` also writes a `velopack/` folder next to the packages in the build artifact (`inlet-windows-x86_64-packages`, `inlet-linux-x86_64`). It holds the full package (`*-full.nupkg`) and the release feed (`releases.<channel>.json`). Publishing an update feed means keeping those files with each release and passing the previous ones to `vpk` as input; the package layout does not change.

### Running the AppImage

The AppImage uses the standard AppImage runtime, which needs FUSE 2. Ubuntu 24.04 does not install it by default: run `sudo apt install libfuse2t64` (older releases: `libfuse2`), or start the image with `./inlet-<version>-linux-x86_64.AppImage --appimage-extract-and-run`, which needs no FUSE. The CI smoke test uses the second form.

### Deep links and login item

The app registers `inlet://` and its login item itself when it starts (`crates/inlet-platform/src/deep_link.rs`, `autostart.rs`). Inside an AppImage the running executable is in a temporary mount, so both register the `.AppImage` file instead, taken from the `APPIMAGE` variable the AppImage runtime sets (`launch_path.rs`). The desktop entry also declares the scheme so that tools that integrate AppImages register it.

On Windows the uninstaller removes the install folder, the shortcut and the uninstall entry, but not the `inlet://` keys under `HKCUSoftwareClasses` that the app wrote itself; they point at a deleted file. Removing them belongs in the Velopack uninstall hook, which comes with the auto-update work.

### Taskbar identity

The Start menu shortcut carries the AppUserModelID `no.opland.inlet` (`--aumid`), the same id the running app sets, so notifications and taskbar grouping use one identity.
