# The Commonty app

The same crates as `dd`, behind a window: Tauri 2, Rust core, plain pages in
`web/`. The device key lives in the OS keystore under the service
`commonty-app`, so the app beside `dd` on one machine is a device of its own.

Build and run from the dev shell:

    cargo build -p commonty && target/debug/commonty

On Ubuntu with nix's toolchain the webview cannot find the host's GL driver;
point it at nix's mesa:

    M=$(nix build --no-link --print-out-paths nixpkgs#mesa)
    __EGL_VENDOR_LIBRARY_FILENAMES=$M/share/glvnd/egl_vendor.d/50_mesa.json \
    LIBGL_DRIVERS_PATH=$M/lib/dri LIBGL_ALWAYS_SOFTWARE=1 target/debug/commonty

`nix build .#app` makes the wrapped binary.

Milestone 1: sign in. Type a name, the app shows `dd device admit <key>`,
run that on a machine that is already yours, and the app is a device in
your entry with every library key sealed to it.

Milestone 2: Movies & TV. The site's own page and player, carried in the
app (`app/carried`); the box transcodes what the webview cannot play
(box/transcode).

Network: the fleet's own network (Headscale on the gateway box). Join asks
the gate for a single-use key in your name and hands it to the engine,
Tailscale's tsnet as a C archive (`app/net`, the one Go in the tree, built
by `nix build .#net` and linked by `build.rs` from `COMMONTY_NET_LIB_DIR`).
From then on every request the app makes goes through the engine's
loopback proxy, by the box's name, straight to the box; state lives in
`~/.local/share/commonty/net` and is resumed at the next start.

Sign-up: "New here? I have an invite code". The app fetches the invite the
code derives, makes the root and recovery keys on this device, publishes
the entry with its grant (`client/account`, shared with `dd identity new
--code`), shows the recovery key once, and joins the network.

Devices: the device that made the account (it holds the root) adds another
by pasting the key from that device's "Add this device" page, and removes
one; every library key follows. "Lost every device?" takes the recovery
key, installs a new root on this device, signs every old device out and
shows a new recovery key.

The engine's proxy is our own SOCKS5 (`app/net/socks.go`, `socks5h` from
the Rust side): its dialer resolves names through the network's DNS first,
where the fleet's names live. `COMMONTY_NET_DEBUG=1` turns the engine's
log on and prints the proxy credential for `curl -x socks5h://tsnet:…`.

## Android

The same app, from the same tree. What differs is at the bottom: the keys
are a file in the app's private storage (`auth::open_file`; Android has no
OS keyring), the engine is a shared library per abi (`nix build
.#net-android`, since Go makes no archives there; it reads interfaces
through `getifaddrs`, the call Android permits), and the app's data directory is what Android gives it.

Build, from `app/`, in the Android shell:

    nix develop ..#android
    mkdir -p gen/android/app/src/main/jniLibs
    cp -r $COMMONTY_NET_ANDROID/* gen/android/app/src/main/jniLibs/
    COMMONTY_NET_LIB_DIR=$PWD/gen/android/app/src/main/jniLibs \
      cargo tauri android build --apk --target x86_64 --debug   # emulator
    COMMONTY_NET_LIB_DIR=$PWD/gen/android/app/src/main/jniLibs \
      cargo tauri android build --apk --target aarch64            # a phone

The apk lands under `gen/android/app/build/outputs/apk/`. `gen/android` is
the project Tauri generated (`cargo tauri android init`), checked in; its
builds and the copied libraries are not. An emulator:

    avdmanager create avd -n probe -k "system-images;android-34;google_apis;x86_64" -d pixel_6
    emulator -avd probe -no-window -gpu swiftshader_indirect &
    adb install -r gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk

Not in ci: a phone build is made here. Proven 2026-09-24 on the emulator:
recovery of an account, sign-in, joining the network through the bridge,
node1 as a peer.

## Windows

The same app again. The engine is a dll rather than a static archive,
because cgo builds with mingw and Rust links with msvc and neither will
take the other's archive; a dll is loaded by name and the question does
not arise. The drive is rclone over WinFsp instead of FUSE, and killing
rclone is enough to tear it down, so there is no `fusermount` there.

The installer is built by `.github/workflows/release.yml` on a tag and
carries the app, `rclone.exe`, and WinFsp's own unmodified installer.
WebView2 is **not** bundled: the installer fetches it from Microsoft if
the machine lacks it, because embedding it would mean shipping
proprietary software beside WinFsp, which its FLOSS exception forbids.

WinFsp - Windows File System Proxy, Copyright (C) Bill Zissimopoulos,
<https://github.com/winfsp/winfsp>. Bundled under the exception it grants
to software under an OSI licence; Commonty is AGPL-3.0-or-later.

### Cross-building Windows from Linux, and what it proves

`dd.exe` cross-compiles with mingw and runs under wine: it makes an
identity, signs the entry, publishes it to a box over tls and mints a
device token, and the box verifies that signature. The whole client
stack works on Windows and is not a guess.

The app cross-compiles too, but only with `crate-type = ["lib"]`. With
the `cdylib` that Android needs, `ld.bfd` fails with `export ordinal too
large` - a limit of the GNU linker, not of the code. GitHub builds on
real Windows with msvc, which has no such limit, so leave the crate
types alone; if that build ever fails the same way, this is why and
dropping cdylib for the desktop target is the fix.

The window itself cannot be tested here at all. Tauri renders through
WebView2, wine has none, and the app starts and then waits with a blank
screen. Proving the window needs Windows.
