# Packaging

How the release files are built. The macOS bundle has its own scripts in `macos/`; the notes below cover Windows and Linux.

## Packages

Both are built with [Velopack](https://velopack.io) from the release-profile binary. CI builds them in the `windows-release` and `linux-release` jobs of `.github/workflows/build.yml`: on pushes to `v0`, on `main`, on tags (through `release.yml`) and on manual runs with `release` ticked. Pull requests do not build them. The version is the one in `[workspace.package]` of `Cargo.toml`. Nothing is code-signed yet.

| File | What it is |
|---|---|
| `inlet-<version>-windows-x86_64-setup.exe` | Installer. Installs for the current user into `%LocalAppData%\no.opland.inlet`, needs no administrator rights, adds a Start menu shortcut and an entry in "Apps & features". |
| `inlet-<version>-windows-x86_64.zip` | Portable copy. |
| `inlet-<version>-linux-x86_64.AppImage` | AppImage (Velopack's Linux package). Make it executable and run it. |
| `inlet-<version>-linux-x86_64.flatpak` | Flatpak bundle (see Flatpak below). Install it with `flatpak install --user ./inlet-<version>-linux-x86_64.flatpak`. |
| `inlet-<version>-linux-x86_64.tar.gz` | The binary, `LICENSE` and `THIRD-PARTY-NOTICES`. |

`release.yml` copies these five files into the release and lists them in `SHA256SUMS.txt`; each pattern must match exactly one file or the release stops.

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
| `linux/no.opland.inlet.metainfo.xml` | AppStream metadata (name, description, licence, links, screenshot, content rating). The id is the app id, like the desktop entry. |
| `linux/flatpak/no.opland.inlet.yml` | Flatpak manifest: the GNOME runtime, the sandbox permissions, and the install steps for the staged files. |
| `linux/flatpak/build-flatpak.sh` | Stages the binary, the desktop entry, the metainfo (release entry stamped with the version and the commit date), the icons and the licences, validates the metadata with `appstreamcli` and `desktop-file-validate`, runs `flatpak-builder` and writes the bundle. |
| `linux/flatpak/smoke-test.sh` | Installs the bundle, runs it (`--version`, then a window under Xvfb for 10 seconds), checks the permissions against the expected list and checks what the app exports. |

The binary answers `--version` (prints `inlet <version>`) and the installer's `--veloapp-*` hook arguments (exits at once) before it opens a window; see `crates/inlet-app/src/main.rs`.

### Updates

The packages are laid out for Velopack auto-update, but nothing in the app checks for updates yet. `build-packages.sh` also writes a `velopack/` folder next to the packages in the build artifact (`inlet-windows-x86_64-packages`, `inlet-linux-x86_64`). It holds the full package (`*-full.nupkg`) and the release feed (`releases.<channel>.json`). Publishing an update feed means keeping those files with each release and passing the previous ones to `vpk` as input; the package layout does not change.

### Running the AppImage

The AppImage uses the standard AppImage runtime, which needs FUSE 2. Ubuntu 24.04 does not install it by default: run `sudo apt install libfuse2t64` (older releases: `libfuse2`), or start the image with `./inlet-<version>-linux-x86_64.AppImage --appimage-extract-and-run`, which needs no FUSE. The CI smoke test uses the second form.

### Deep links and login item

The app registers `inlet://` and its login item itself when it starts (`crates/inlet-platform/src/deep_link.rs`, `autostart.rs`). Inside an AppImage the running executable is in a temporary mount, so both register the `.AppImage` file instead, taken from the `APPIMAGE` variable the AppImage runtime sets (`launch_path.rs`). The desktop entry also declares the scheme so that tools that integrate AppImages register it.

On Windows the uninstaller removes the install folder, the shortcut and the uninstall entry, but not the `inlet://` keys under `HKCUSoftwareClasses` that the app wrote itself; they point at a deleted file. Removing them belongs in the Velopack uninstall hook, which comes with the auto-update work.

### Taskbar identity

The Start menu shortcut carries the AppUserModelID `no.opland.inlet` (`--aumid`), the same id the running app sets, so notifications and taskbar grouping use one identity.

### Flatpak

The bundle packs the release binary that the Linux job built, so it is the same program as the tarball and the AppImage. It runs on the GNOME runtime (branch 51) because that runtime ships WebKitGTK 4.1, which the embedded web helper `inlet-web` needs; the build installs the helper next to `inlet` whenever it sits next to the binary in `dist/`. The runtime is not in the bundle: `flatpak install` fetches it from Flathub (the bundle names the Flathub repository). When the GNOME branch reaches end of life, change `runtime-version` in the manifest.

Build and test it on a machine with `flatpak`, `flatpak-builder` and `appstreamcli`, and a Flathub remote or network access (the build adds the remote for the current user):

```sh
packaging/linux/flatpak/build-flatpak.sh target/release/inlet out
packaging/linux/flatpak/smoke-test.sh out/inlet-<version>-linux-x86_64.flatpak <version>
```

Sandbox permissions, each with its reason in the manifest: network; Wayland, X11 only when there is no Wayland, and shared memory for X11; GPU (`dri`); PulseAudio for the notification sound; the session bus names for notifications, the tray (`org.kde.StatusNotifierWatcher`, and owning the tray item `org.kde.StatusNotifierItem-2-1`), the secret service (the sign-in token) and idle time (`org.freedesktop.ScreenSaver`, `org.gnome.Mutter.IdleMonitor`). There is no file system access: the file chooser, saving files and opening links go through the desktop portals. The smoke test compares the permissions with this list, so widening them means changing the manifest and the test together.

What does not work the same inside the sandbox: the app writes its own `inlet://` handler and login item under its own (sandboxed) home, which the host does not read. The bundle's exported desktop entry declares `x-scheme-handler/inlet`, so links open the app, but "start at login" needs the Background portal (the comment in `crates/inlet-platform/src/autostart.rs` says it is not done). The tray item name assumes the app is process 2 in the sandbox (checked) and that one tray is made per run.

#### Flathub

Nothing is submitted to Flathub. A Flathub submission is a separate manifest in a pull request to `flathub/flathub`, and it must build from source (no prebuilt binary), so this manifest is the model for the sandbox and the metadata, not the file to submit. Before submitting:

- a source build: a tagged source archive, the Rust SDK extension (`org.freedesktop.Sdk.Extension.rust-stable`) and an offline set of Cargo sources (`flatpak-cargo-generator` over `Cargo.lock`), plus the build dependencies the Linux job installs;
- a `<release>` entry in the metainfo for the tag (here it is stamped when the bundle is built) and a screenshot taken on Linux, at a URL that stays valid (the current one is the Windows demo picture on `v0`);
- proof that the app id belongs to us: Flathub checks that `https://opland.no` answers for an id under `no.opland` and asks for the domain verification file (or the id changes to the `io.github.<user>.Inlet` form, which would also mean new names for the desktop entry and icons);
- `flatpak-builder-lint` on the manifest, the repository and the metainfo (a local run reports only the domain check and the screenshot mirroring, which Flathub's own build does), and the Flathub reviewers' answer on the permissions above (the tray item name and the two idle-time bus names are the likeliest questions);
- the maintainer's decision to submit.
