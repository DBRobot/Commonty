# The app

The Commonty app is what a member installs: a window onto their boxes for
Linux, Windows and Android. It is the same Rust crates `dd` is made of
(`client/`), behind a Tauri window, with the fleet's network built in.
Its code is in [`app/`](../../app/); how it is built and released is
[its own page](app-release.md).

## What it does

- **Is a device of its own.** It keeps one device key in the system's
  keystore (a file in the app's private storage on Android). An account
  admits that key into its signed entry, and from then on every request the
  app makes is signed by it: no password, no browser session.
- **Joins the fleet's own network.** The engine is Tailscale's tsnet, in Go
  ([`app/net/`](../../app/net/)), built into the app. Joining asks the gate for
  a single-use key in the member's name; the membership then lives on disk
  and resumes at every start. Boxes are reached directly by name, not
  through the public door.
- **Carries the site's pages.** The signed-in pages (home, Files, Movies &
  TV, Settings) ship inside the app and are served to its own window
  ([`app/src/site.rs`](../../app/src/site.rs)), so they open without a round
  trip. What they ask a box goes over the network with the device's token.
  The copies in `app/web/_dd/` are the site's own files, byte for byte; a
  check in the flake fails the build when one differs (`app/carried` lists
  them).
- **Lets the browser in too.** On a desktop, the system's browser is pointed
  at the fleet for the fleet's names alone (Windows' Internet Settings,
  GNOME's proxy setting), through a proxy the app runs. On a phone, a VPN
  does the same for the whole device.

## How it is kept safe

The app holds keys that speak for a person. What reaches them is narrow:

```
the app's window
  ├─ its own pages ──── carry a secret made at each launch; only they may
  │                     ask for keys (/_dd/app/...)
  ├─ anything else ──── refused by origin before a key is near it
  └─ a box's answers ── data only: html, script, svg and css are refused,
                        so nothing a box sends can run as the app
the browser's proxy ─── no password (browsers cannot send one), so it
                        answers only the account running the app, checked
                        against the system's table of connections, and
                        reaches the fleet's names and nothing else
```

## Where things are

```
app/src/      the window and its commands
  site.rs       the carried pages, and the proxy to the boxes
  account.rs    sign-up, recovery, joining by code, a browser passkey
  net.rs        the engine, through four C calls
  browser.rs    the system browser's proxy setting
  photos.rs     Photos' sign-in, handed over from the browser once
  vpn.rs        Android's VPN
app/net/      the engine (Go): tsnet, its proxies, the bridge through
              the front door, the owner check
app/web/      the app's own few screens, and the carried site pages
app/gen/      the Android project Tauri made, and our VPN plugin
```

Its state is in `~/.local/share/commonty` on Linux and the platform's own
app data folder elsewhere (AppData on Windows). It joins the network under
the machine's name, or `phone-xxxx` where there is none.

## The app's own screens

They are only what the site cannot do: choosing a name, joining with a
code from another device, signing up with an invite, recovering an
account, connecting to the network, setting up a passkey in this
computer's browser, and signing out on this device. Everything else -
devices, passkeys, storage, the services - is the site's own pages, which
the app carries.

## Checks

- CI runs the app's lint and tests (`checks.clippy-app`,
  `checks.tests-app`) and the engine's `go vet` and tests
  (`checks.net-check`), whenever the app or what it is built from changes.
- Every download is built by two boxes after a release that changes the
  app ([app-release.md](app-release.md)): a broken or unreproducible one
  stops there, and never reaches the downloads page.
- The window itself cannot be driven in CI. A change to what it shows is
  looked at by hand, on the platforms it touches, before it merges.

## Changing it

- **A site page the app carries:** change it under `servers/`, copy it to
  `app/web/_dd/` in the same pull request (the flake's check will say which
  differ). The app then picks it up at its next release.
- **Something only the app does** belongs in `app/src/` and the app's own
  screens; anything a browser could do belongs on the site.
- **Anything that touches keys** goes behind the launch secret
  (`/_dd/app/...`), and its pull request says so in "Risk and rollback".
- The [CI values](ci.md#adding-to-ci) hold here too: complete, and fast.
