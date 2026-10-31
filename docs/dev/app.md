# The Commonty app

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

## It keeps running

The key pages a browser opens are served by the app, so a browser sent to
them while the app is not running finds nothing. The app therefore stays
running until the member quits it
([`app/src/background.rs`](../../app/src/background.rs)):

- **Closing the window hides it.** The tray icon's menu has Open Commonty
  and Quit; Quit, or the system ending the process, is the only way it
  stops. The tray is Tauri's own (`tray-icon` feature). On Linux it needs
  libayatana-appindicator: the `.deb` depends on it, the AppImage carries
  it, and the Nix build puts it on the library path. Where none loads (a
  GNOME without the AppIndicator extension shows no icon either), there
  is no tray, and launching the app again brings the window back.
- **It starts at login, hidden, and comes back.** At every start the app
  writes its login entries, or removes them, as the setting says. All of
  them start it with `--background`: no window, the tray and the servers
  alone. The window is made hidden (`tauri.conf.json`) and shown in
  `setup` unless the start was `--background` or a `commonty://` link.
  - **Linux, with a systemd user manager:** a user unit,
    `~/.config/systemd/user/commonty.service`, enabled by hand into
    `graphical-session.target.wants` (no `systemctl enable` at every
    start; `daemon-reload` only when its text changed). `Restart=always`
    with `RestartPreventExitStatus=0`: a crash, a kill or `SIGTERM` brings
    it back after 2 s, Quit (exit 0) and a stop (logging out, the setting
    turned off) do not; five starts in a minute and systemd gives up. It
    is `Type=dbus` on single-instance's bus name
    (`org.commonty.app.SingleInstance`), so `systemctl --user start`
    returns once that copy can take a second launch's arguments. Started
    any other way (the menu, a link, an autostart entry the session read
    itself), the app first starts the unit, if it names this very program,
    then hands its arguments over through single-instance and ends
    (`into_unit`): the copy that runs is the one systemd watches. A debug
    build never does. The XDG autostart file
    (`~/.config/autostart/commonty.desktop`, naming the AppImage's own
    file when it is one) stays for a session without systemd; with the
    unit in place it says `X-systemd-skip=true`, so a session that turns
    autostart files into units (GNOME, Plasma) leaves it to the unit. The
    `.deb` ships nothing for this: the app writes both for its user.
  - **Windows:** the per-user `Run` key (the uninstaller removes it, an
    update keeps it) starts `commonty.exe --keep`, the keeper: no window
    and no Tauri, only the app as a child with `--background`, started
    again after 2 s when it ended by anything but Quit, and given up on
    after five ends in a minute.
  - **macOS:** `~/Library/LaunchAgents/org.commonty.app.plist`, with
    `RunAtLoad` and `KeepAlive` unless the exit was successful. No macOS
    build is published yet, so this is written but untried.
- **A link wakes it.** A `commonty://` link to a running app goes to it
  through single-instance (Linux, Windows and macOS); started cold by the link, it comes up hidden.
  Either way the window shows only at "Link this browser", the click a
  new browser's pairing needs; a browser let in before opens its page
  with no window at all.
- **The setting** is "Start Commonty when you sign in to this computer"
  on the app's own screen for this device, on unless turned off. It is
  the file `start-at-login` in the data folder (`off` turns it off),
  which the Android side reads too.
- **Android** keeps the process alive with a foreground service
  ([`KeepRunning.kt`](../../app/gen/android/app/src/main/java/org/commonty/app/KeepRunning.kt)),
  type `specialUse` (none of Android's own types is serving pages to the
  phone's browser), with a silent notification. The app's screen starts
  it, and `StartAtBoot.kt` starts it after a reboot or an update when the
  setting is on; turning the setting off stops it. The Rust side only
  starts with the app's screen, which Android does not let a background
  app open, so after a reboot the notification says "Commonty is ready"
  until the member taps it, and "Commonty is running" from then on. A link
  from the browser opens the screen anyway. The first time, the app asks
  for notifications (Android 13 and later) and once, in its own words, to
  be left out of battery saving; it carries on either way.

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

## Keys come only from devices

No node is trusted, and a passkey is no key. Root on any box can change
any page under `*.<domain>`, and a passkey answers its PRF, with any salt,
to any page of the domain during an ordinary prompt. So:

- **A passkey signs in, and that is all.** No key is made from one or
  sealed to one, and no page asks one for its PRF but the one-time move of
  an old Photos account ([packaged.md](packaged.md#sign-in)), whose answer
  opens nothing once moved. A sign-in signature is not key material.
- **Keys are this device's.** Library keys, the Photos password and the
  archive password are sealed to device keys (the app's, dd's), a root
  kept on one, and the paper key ([libraries.md](libraries.md#where-the-keys-live)).
  The app opens them with its own key, out of the entry it checks under
  the root it holds or pinned the first time (`account::own_entry`), and
  that first time is held to the release's signed member list, not taken
  on sight; a shared secret is taken only when a device of that entry
  signed it.
- **Every page that holds a key is the app's.** Files, Movies & TV,
  Chat, Settings (Backups, device names), Photos and Passwords come in
  the app's signed release, in its window and in the member's browser. A
  box hands a member's browser to the app instead of serving them.
  Settings from a box is given no key: one button there hands those
  parts to the app (shell.js `throughApp`), which opens its own copy of
  Settings in that browser.
- **One sign-in.** The browser signs in once with its passkey at the gate
  (the session every box page and the vault's single sign-on take). What
  the app serves asks the boxes with its device's token, and the app
  hands its pages their keys itself: no second prompt anywhere. Letting a
  browser in from the app is a click in the app, not a sign-in.
- **A member is made in the app.** Sign-up with an invite code is the
  app's (or `dd identity new --code`): the root, the paper key and the
  code stay on the device. The site's own sign-up makes a passkey-only
  account with no code (a friend's guest), which holds no keys.

At each launch the app moves what came before
([`site::at_launch`](../../app/src/site.rs)): where the root is on this
device, every library is sealed again to devices, root and paper key alone
(`account::reseal`), and Photos' password is shared with the member's
other devices. What that does not undo is in
[libraries.md](libraries.md#moving-off-passkey-keys).

An account whose root is a passkey (made in a browser) is the one gap: its
entry is signed by an assertion, which a box's page can ask for over an
entry the box wrote. Such an account holds no library (only a root on a
device makes one). Its Photos password is kept on one device alone, and
it has no paper key to recover with: its recovery is that device. Moving
such a member to a root on a device (the app making a root and a paper
key, signed in once by the passkey) is not built yet.

## The site's key pages, Photos and Passwords in a browser

Ente's photos client, the web vault, and the site's pages that open a
library or another key hold a member's keys, so their code comes only in
the app's signed release ([packaged.md](packaged.md)). No box serves it to
a member. The app serves them from this computer
([`app/src/local.rs`](../../app/src/local.rs)), to its own window and to
the member's own browser: Photos on port 41661, Passwords on 41662, and
the site's pages (Files, Movies & TV, Chat, Mail, Settings) on 41660. A
browser is linked only when the member allows it from the app's window,
once, for every service: the app remembers it until the member forgets it. The address the app opens
the browser at holds no secret. The bar on Photos there (`p-<own>`) and
on Passwords (`v-<own>`) goes to the site's pages at `s-<own>`, in the same
tab, never to a box's copy (`shell.js`, `here`).

The site's pages there are the very files the window carries
(`app/carried`), served at `s-<own>.localhost:41660`; `/_dd/app/library`
and `/_dd/app/archive` answer a let-in browser alone (`handoff::admit`:
the key is behind `Serve`, so behind that browser's cookie), so do the Leave
tab's downloads (`/_dd/app/export`, each job its own browser's;
[leaving.md](leaving.md#taking-things-out)), every other
call goes to the boxes with the device's token, Chat's to the model box,
and nothing a box answers is code, nor any cookie but the vault's
sign-in binding (`VW_SSO_BINDING`, host-only on
`v-<own>.localhost`, sent back on `/identity/connect/` alone). A folder's
listing is the one XML that passes: WebDAV's 207, to a PROPFIND, with the
sandbox every answer gets (`site::data_from_box`). The box's own sign-in
page never does: `/_dd/login` and `/_dd/logout` are answered here, with a
401 that says the device's sign-in was not taken. Adding or
removing a device with the root stays the window's, and so does leaving
(`/_dd/app/leaving`, signed with the root here), which deletes everything
at once and cannot be undone ([leaving.md](leaving.md)). From one of these
pages, Photos and Passwords open through `/_dd/app/go`, a door of their
own for the same browser, without another click.

### One key route

A page asks for the keys it needs with one call, in the window or a
browser the app let in: `POST /_dd/app/keys {services: [...]}`
([`app/src/keys.rs`](../../app/src/keys.rs)). It answers each service with
its key or `{error}`. The services are `library` (the library key, as
`/_dd/app/library`), `mail` (the mail key, as `GET /_dd/app/mail/key`) and
`photos` (Photos' password, which the app made and keeps). `vault` is
refused: Passwords' key goes only to Passwords' own origin. The route sits
behind the same checks as the others. In the window that is the launch
secret, and in a browser it is `admit`'s `Serve`, so a let-in browser's
cookie on a same-origin request. Every page served at the site's origin can
ask for every service it lists, because they are one origin and the app
cannot tell one carried page from another. What keeps a box's or a
message's code out is that nothing a box answers runs there (above) and
that a message's HTML runs no script (mail.md). Mail asks for Photos this
way; the older per-service routes still answer until every page has moved.

A page with Photos' password may reach only its own origin, so the app
passes on its calls to museum (`/_dd/ente/<path>` to `api.<domain>`) and
to the store (`/_dd/ente-store/<host>/<path>`, for `photos-store.<domain>`
or `s3.<domain>` alone). The app sends only the headers Ente's calls carry,
no token of the device's. It turns museum's redirect of a download into one
to `/_dd/ente-store/...`, and lets nothing through that would run.

```
photos.<domain> in a browser ─ the gate's handoff page (/_dd/use-app)
  [Open with the Commonty app], or by itself once the dd_app note is set
                ─ commonty://open?service=photos&path=/gallery
                     │ the OS starts the app, or hands the link to the copy
                     ▼ already running (single-instance)
the app ─ arms Photos' pairing for 120 s, opens the default browser at
          http://s-<install>.localhost:41660/_dd/app/pair?service=photos
          (nothing secret; every service pairs on the site's page)
the browser ─ pair.js: let in before? (localStorage "dd-own" on this origin)
              yes ─ POST /_dd/app/pair/known {host} ─▶ still this device's name
                  ─ http://s-<own>.localhost:41660/_dd/app/resume?service=photos
                  ─▶ (cross-site, so no Strict cookie yet) a page that opens
                     itself again ─▶ the cookie checks: the link's page, through
                     a door into Photos (below). No code, no click, no window.
              no  ─ a random verifier, kept in the tab
                  ─ POST /_dd/app/pair/ask {hash: sha256(verifier)} ─▶ a 6-digit code
the window ─ comes up at "Link this browser?" with the same code (only when a
             browser asks); the member clicks "Link this browser"
the browser ─ POST /_dd/app/pair/finish {verifier} ─▶ the hash matches and it
              was allowed: remembered (browsers.json), a door secret
            ─ http://s-<own>.localhost:41660/_dd/app/enter?k=<one-time, 60 s>
              ─▶ dd_browser cookie (HttpOnly, SameSite=Strict, 400 days),
                 a page that goes on by itself (so the next request is same-site)
            ─ /_dd/app/resume?service=photos ─▶ a door into Photos the same way,
              http://p-<own>.localhost:41661/_dd/app/enter?k=…, its own cookie
            ─ /_dd/photos: POST /_dd/app/photos-key, the password once
            ─ Ente's client, signed in
```

- **The member's click, once.** The app's window shows "Link this
  browser?" and the code. The browser's page shows the same code. "Link
  this browser" lets in that one request, for every service, and "Not me"
  refuses it. If a second request comes
  while one is waiting, or after the click but before the browser
  finishes, both are refused. Both screens then say that two browsers
  asked at once, and the member opens it again from the page. Nothing can
  ask unless a link armed the pairing in the last 120 seconds. After that
  the browser is remembered: later links open the page in it straight
  away, across the app's restarts, with no code, no click and no window.
- **Two kinds of name.** `<install>` is 32 random hex characters, made at
  the first run and kept in the data folder (`local-names`, 0600). It is
  **not secret** from other programs on the device: it is in the command
  line the browser is started with, and on Linux every user can read
  that. So nothing is let in there. It serves only the pairing page and
  its asks. `<own>` is this device's name for the browsers it lets in:
  32 hex characters of a hash of the device key (`handoff::own_label`), so
  the same across restarts and new with a new device, and telling nothing
  of the key. It goes to a browser only in the answer to its own
  same-origin request, and is in no command line. The browser's cookie,
  and what Ente or the vault keep in the browser, are under that name, so
  they stay put across restarts. The app's window keeps `127.0.0.1`.
- **A remembered browser** is a line in `browsers.json` (data folder,
  0600, for the device key's fingerprint: a new key starts empty): an id,
  "Chrome on Linux", when it was let in and last used, and for each
  service the SHA-256 of its cookie's secret. The cookie is
  `dd_browser=<id>.<secret>`, one secret per service's name. Up to 32
  are kept; the one longest unused goes first. **This device** in the
  app's window lists them, each with **Forget**; signing out in that
  browser (the bar's Sign out) forgets it too, and signing out on the
  device forgets them all. A forgotten browser's cookie opens nothing at
  once, and its next link asks for a code again (`?again`).
- **A tab opened from elsewhere** (a bookmark, a link, the pairing page)
  carries no Strict cookie and says it is cross-site. A top-level GET of
  a document then gets a page that opens itself again (`Admit::GoOn`),
  and nothing else: the next request is same-origin, with the cookie.
  Frames, fetches, forms and anything but a GET are refused as before.
- **Browsers** resolve every `*.localhost` to loopback themselves (Chrome,
  Edge, Firefox) and treat it as a secure context, so WebCrypto works on
  the pairing page.
- **The browser's proxy** (the PAC file in
  [`app/net/browser.go`](../../app/net/browser.go)) sends only the fleet's
  domain to the app and everything else `DIRECT`, so `*.localhost` never
  goes through it. Browsers also never proxy loopback names.
- **The vault's sign-in.** The browser signs in at the gate with its own
  passkey on `home.<domain>`. Vaultwarden sends it back to
  `v-<own>.localhost:41662/sso-connector.html`
  ([the patch](packaged.md#passwords-vaultwarden)). That one page, a
  door and `/_dd/app/resume` are served without the cookie, because the
  way in is from another site and a Strict cookie is not sent then. What
  they load after that is same-site and carries the cookie.
- **The first time on a device**, Photos has no password in the keyring
  yet. The app takes it from the copy beside the member's disk images,
  signed by another of their devices; failing that it makes one and the
  Photos page makes the account with it, or moves an account a passkey
  made ([packaged.md](packaged.md#sign-in), photos.rs).
- **The network comes up a few seconds after the window.** A page's
  first calls wait for it on a device that has joined, and a call that
  finds no way through yet is sent again (`local::send`), so a page opened
  at once is not a 502. A 502 says why in the app's log.
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
                                    at most once a second, and only service=photos|vault|site
                                    with a plain path (a_link_names_a_service_and_a_path_on_it)
a local program reads the           the address holds no secret, and never this device's
  browser's command line            own name (the_launched_url_holds_no_secret); asking
                                    with the install's name gets a code the member never
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
  port reading the cookie           under this device's own name, which no other
                                    program is told, so the browser never sends it there
the origin's storage read while     same: what Ente and the vault keep is under that
  the app is not running            name, never the install's. But see "Remembered,
                                    and what that costs" below
DNS rebinding, other hosts          the Host header must be exactly 127.0.0.1:<port>,
                                    the install's name, or this device's own name
                                    (only_the_exact_host_and_port_are_answered)
CSRF from other pages               SameSite=Strict, and from a browser a change needs
                                    Origin equal to the page's own; anything said to
                                    come from another site is refused, except the ways
                                    in and a tab opening a page, which gets a page that
                                    opens itself again (changes_come_only_from_this_origin,
                                    only_a_browsers_ways_in_come_without_a_cookie,
                                    a_page_opened_from_elsewhere_opens_itself_and_nothing_more)
a website fires commonty:// at a    the member's own browser opens one of their pages,
  remembered browser                as a link to it would; the site gets nothing back and
                                    cannot read or drive the page (another origin)
a door reused or late               once, 60 s, for its own service and browser
                                    (a_door_opens_once_in_time_for_its_service)
a forgotten browser                 its cookie opens nothing at once, its doors go, and
                                    it asks for a code again (a_forgotten_browser_opens_nothing)
a remembered browser after a        still let in, at the same name; a new device key
  restart                           starts with none (a_browser_let_in_is_remembered_across_restarts)
an open redirect through the path   a path on the service's own origin, in a short safe
                                    alphabet (then_is_a_path_on_this_origin_only,
                                    the_door_page_goes_on_to_its_path_and_nowhere_else)
the Photos password in history      in no address: the page asks for it once, within
  or synced history                 60 s of its door, same-origin, with its browser's
                                    cookie (the_photos_password_is_handed_once_...)
a key asked of a browser no one      the library, the archive and Photos' password are
  let in                            behind Serve: the browser's cookie, given after the
                                    member's click (keys_are_served_only_to_a_browser_the_member_let_in)
a link to the site's pages          only Files, Movies & TV, Chat, Settings, home
                                    (a_link_can_ask_for_the_sites_own_pages_and_nothing_else)
a copy of a secret a box sealed     taken only signed by a device of the root-checked
                                    entry (account: a_copy_a_box_made_is_refused)
a javascript: link in the bar       the bar links only paths here and the fleet's own
                                    names, by the domain the app writes into its pages
the box sends code                  refused, as for the window (site.rs,
                                    a_box_sends_data_and_never_a_page,
                                    a_folders_listing_passes_and_a_page_from_the_box_does_not)
a file the app does not carry       a 404: Tauri's own lookup answered with the app's first
                                    screen (site::carried)
```

The default browser is trusted with what it is opened at, as it already is
with the member's passkeys. On Android, that is whichever app the member
chose to open web links. A program that can read the member's own files
or the browser's profile can already read the app's keys too, as it could
before. None of this stands against that.

### Remembered, and what that costs

Pairing once means something in the browser outlives the app's run: the
cookie, and on the pairing page's origin the device's own name (pair.js,
`dd-own`). What that adds, plainly:

- **A program that takes the port while the app is closed.** Anything on
  the computer can listen on 41660 when the app is not running. If the
  member's browser is then sent to the install's pairing address (it is
  in old command lines, so another account on the computer can know it,
  and a web page that knows it can open it), that program reads `dd-own`
  there, can send the browser on to the device's own name on the same
  port, and gets the cookie. With it, once the app is back, it could ask
  for the library's key. Before, there was nothing kept to find. What
  stands against it: the app opens a browser only at a port it holds;
  the app keeps running unless quit ([above](#it-keeps-running)); and
  **Forget** in **This device** ends a cookie at once. The same program could read the
  app's keys outright if it ran as the member, so this is about other
  accounts and sandboxed apps on the same computer.
- **The browser's history** has the device's own name in it, as it had a
  session's before; that name now lasts. It is no key: the cookie is.
- **A website** can still only fire a link: the member's own browser opens
  one of their pages, as a bookmark would. Nothing comes back to the site,
  and every own-request check above still holds.
- **The `dd_app` note** (a cookie on the whole domain, 400 days) is set by
  the handoff page once the app has taken its link, and makes later
  handoff pages fire the link by themselves, and a signed-out member's
  Files, Movies & TV, Chat and Mail show the handoff page rather than the
  box's sign-in. It holds nothing and lets nothing in: anything that can
  set it (any page on the domain) only makes a page fire the app's own
  link, which the system hands to the installed app alone. The page never
  sends a tab to a port itself.

## Where things are

```
app/src/      the window and its commands
  site.rs       the carried pages, and the proxy to the boxes
  local.rs      what holds keys in a page (Photos, Passwords, the site's
                key pages), served to the window and the browser
  handoff.rs    pairing a browser, the browsers it remembers, names, links, admit
  account.rs    sign-up, recovery, joining by code, a browser passkey
  photos.rs     Photos' password: made here, shared signed, the move
  export.rs     the Leave tab's zips: every photo, made here; a library's,
                made in the page and carried to a download or a file
  zip.rs        a zip written as it goes
  net.rs        the engine, through four C calls
  background.rs the tray, the login entry and its setting, Android's service
  browser.rs    the system browser's proxy setting
  installer.rs  setting up a home controller, a window of its own (desktop)
  boxsetup.rs   setting up a box: its installer on a USB drive with a
                join ticket, the backup's password and the Wi-Fi; the
                box's backup into the member's images once it joins
                (docs/dev/joining.md), a window of its own (desktop)
  wifi.rs       this computer's Wi-Fi and its password, for that drive
  writer.rs     dd-hub-writer, run elevated, and its progress
  controllers.rs  the household's home controllers, locked with the library key
  smarthome.rs  Smart home: a loopback server held to the controller's key
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
computer's browser, allowing a browser to open Photos or Passwords,
whether the app starts by itself, and signing out on this device.
Everything else - devices, passkeys, storage, the services - is the
site's own pages, which the app carries.

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
  ([above](#the-sites-key-pages-photos-and-passwords-in-a-browser)). That needs a by-hand
  check in Chrome, Firefox and Samsung Internet on the platforms it
  touches.
- The [CI values](ci.md#adding-to-ci) hold here too: complete, and fast.
