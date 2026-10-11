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

## Photos and Passwords in a browser

Ente's photos client and the web vault hold a member's keys, so their code
comes only in the app's signed release ([packaged.md](packaged.md)). No
box serves it. The app serves both from this computer
([`app/src/local.rs`](../../app/src/local.rs)), to its own window and to
the member's own browser. A browser is let in only when the member allows
it from the app's window, and the address the app opens the browser at
holds no secret.

```
photos.<domain> in a browser ─ the gate's handoff page (/_dd/use-app)
  [Open Photos] ─ commonty://open?service=photos&path=/gallery
                     │ the OS starts the app, or hands the link to the copy
                     ▼ already running (single-instance)
the app ─ arms Photos' pairing for 120 s, shows its window at "Allow this
          browser", opens the default browser at
          http://p-<install>.localhost:41661/_dd/app/pair      (nothing secret)
the browser ─ pair.js: a random verifier, kept in the tab
            ─ POST /_dd/app/pair/ask {hash: sha256(verifier)} ─▶ a 6-digit code
the window ─ shows the same code; the member clicks "Allow this browser"
the browser ─ POST /_dd/app/pair/finish {verifier} ─▶ the hash matches and it
              was allowed: a new session, its own name, a door secret
            ─ http://p-<session>.localhost:41661/_dd/app/enter?k=<one-time, 60 s>
              ─▶ dd_browser cookie (HttpOnly, SameSite=Strict, that name only),
                 a page that goes on by itself (so the next request is same-site)
            ─ /_dd/photos: POST /_dd/app/photos-key, the password once
            ─ Ente's client, signed in
```

- **The member's click.** The app's window shows "Allow this browser to
  open Photos?" and the code. The browser's page shows the same code, and
  tells the member to check it in the app. "Allow this browser" lets in
  that one request, and "Not me" refuses it. If a second request comes
  while one is waiting, or after the click but before the browser
  finishes, both are refused. Both screens then say that two browsers
  asked at once, and the member opens it again from the page. Nothing can
  ask unless a link armed the pairing in the last 120 seconds.
- **Two kinds of name.** `<install>` is 32 random hex characters, made at
  the first run and kept in the data folder (`local-names`, 0600). It is
  **not secret** from other programs on the device: it is in the command
  line the browser is started with, and on Linux every user can read
  that. So nothing is kept or let in there. It serves only the pairing
  page and its two requests. `<session>` is new for each browser let in.
  It goes to that browser alone, in the answer to its own same-origin
  request, and is in no command line. The browser's cookie, and what Ente
  or the vault keep in the browser, are under that name. The app's window
  keeps `127.0.0.1`.
- **Browsers** resolve every `*.localhost` to loopback themselves (Chrome,
  Edge, Firefox) and treat it as a secure context, so WebCrypto works on
  the pairing page.
- **The browser's proxy** (the PAC file in
  [`app/net/browser.go`](../../app/net/browser.go)) sends only the fleet's
  domain to the app and everything else `DIRECT`, so `*.localhost` never
  goes through it. Browsers also never proxy loopback names.
- **The vault's sign-in.** The browser signs in at the gate with its own
  passkey on `home.<domain>`. Vaultwarden sends it back to
  `v-<session>.localhost:41662/sso-connector.html`
  ([the patch](packaged.md#passwords-vaultwarden)). That one page, and the
  session's door, are served without the cookie, because the way in is
  from another site and a Strict cookie is not sent then. What they load
  after that is same-site and carries the cookie.
- **The first time on a device**, Photos has no password in the keyring
  yet. A link then opens the app's window at its Photos page, which gets
  the password with the browser's passkey (photos.rs). Links after that
  open the browser.
- **Sessions live as long as the app.** After the app restarts, a browser
  pairs again from the page. Old session names stay in the browser's
  storage, but nothing on the device knows them except that browser
  profile.
- **Links come in** by `tauri-plugin-deep-link`. The `.deb` desktop entry
  and the Windows installer register the scheme. An AppImage registers
  itself at start. On Android it is an intent filter in the manifest. On
  the desktop, `tauri-plugin-single-instance` hands a second launch's link
  to the copy already running.

What each threat is, and what closes it (the tests are in
[`app/src/handoff.rs`](../../app/src/handoff.rs)):

```
another site fires commonty://      it can only make the app open the member's own
                                    browser at the pairing page and arm one pairing;
                                    nothing goes back to the site. Links are taken
                                    at most once a second, and only service=photos|vault
                                    with a plain path (a_link_names_a_service_and_a_path_on_it)
a local program reads the           the address holds no secret
  browser's command line            (the_launched_url_holds_no_secret); asking with
                                    the install's name gets a code the member never
                                    allows, or makes two requests, and both are refused
                                    (two_requests_at_once_are_both_refused,
                                    a_second_request_after_the_click_still_refuses_both)
a request no one allowed            gets nothing (a_request_without_the_members_click_gets_nothing,
                                    the_click_is_for_the_code_shown)
an allowed request, someone else    the session goes only to whoever shows the verifier
  finishing it                      whose hash was allowed; the hash is not enough
                                    (an_allowed_request_is_not_let_in_without_its_verifier)
another program took the port       neither the window nor a browser is sent to a port
                                    this app does not hold (no_browser_for_a_port_this_app_does_not_hold)
a listener on another loopback      cookies ignore ports, but the browser's cookie is
  port reading the cookie           under the session's own name, which no other
                                    program knows, so the browser never sends it there
the origin's storage read while     same: what Ente and the vault keep is under the
  the app is not running            session's name, never the install's
DNS rebinding, other hosts          the Host header must be exactly 127.0.0.1:<port>,
                                    the install's name, or a live session's name
                                    (only_the_exact_host_and_port_are_answered)
CSRF from other pages               SameSite=Strict, and from a browser a change needs
                                    Origin equal to the page's own; anything said to
                                    come from another site is refused, except the two
                                    ways in (changes_come_only_from_this_origin,
                                    only_a_browsers_ways_in_come_without_a_cookie)
a session door reused or late       once, 60 s, for its own service, spent by a wrong try
                                    (a_session_door_opens_once_in_time)
an open redirect through the path   a path on the service's own origin, in a short safe
                                    alphabet (then_is_a_path_on_this_origin_only,
                                    the_door_page_goes_on_to_its_path_and_nowhere_else)
the Photos password in history      in no address: the page asks for it once, within
  or synced history                 60 s of its door, same-origin, with its session's
                                    cookie (the_photos_password_is_handed_once_...)
the box sends code                  refused, as for the window (site.rs,
                                    a_box_sends_data_and_never_a_page)
```

The default browser is trusted with what it is opened at, as it already is
with the member's passkeys. On Android, that is whichever app the member
chose to open web links. A program that can read the member's own files
or the browser's profile can already read the app's keys too, as it could
before. None of this stands against that.

## Where things are

```
app/src/      the window and its commands
  site.rs       the carried pages, and the proxy to the boxes
  local.rs      Photos and Passwords, served to the window and the browser
  handoff.rs    pairing a browser, its sessions and names, links
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
computer's browser, allowing a browser to open Photos or Passwords, and
signing out on this device. Everything else - devices, passkeys,
storage, the services - is the site's own pages, which the app carries.

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
  For Photos and Passwords in a browser, it goes behind the member's click
  ([above](#photos-and-passwords-in-a-browser)). That needs a by-hand
  check in Chrome, Firefox and Samsung Internet on the platforms it
  touches.
- The [CI values](ci.md#adding-to-ci) hold here too: complete, and fast.
