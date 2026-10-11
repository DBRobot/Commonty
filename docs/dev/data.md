# Data the services use

`data/` is data the services are built with: files that are made once,
by a script run by hand, and committed, so a build never reaches out for
them. Today it holds one thing, the game servers' catalogue. This page is
how that is laid out, how it is made, and who reads it. How the game
servers use it at run time is [Game servers](games.md).

## Where things are

```
data/
  games/
    catalogue.json   every game server members can start: 111 entries, and the
                     eggs commit they were made from
    covers/<id>.jpg  each game's cover, by the game's own Steam app id (87 of them)
    resolve.py       pelican eggs -> catalogue.json and covers/
    sizes.py         adds disk_bytes to each entry, from Steam's depot sizes
```

Nothing in `data/` is generated at build time and nothing is fetched by a
build. The scripts reach Steam and GitHub; the files they write are what
the builds read.

## The catalogue

[`catalogue.json`](../../data/games/catalogue.json) is one object:

```
eggs
  repo      the eggs repository the catalogue came from
            (https://github.com/pelican-eggs/games-steamcmd)
  commit    the commit of the checkout resolve.py read (e17e2c3d...)
games[]     one entry per game server, sorted by name
```

`eggs` is the pin. resolve.py writes it from the checkout it was given
(`git remote get-url origin`, `git rev-parse HEAD`) and refuses a checkout
with changes in it, so the catalogue always says exactly which eggs it
came from.

Each entry:

```
id              our id: the name, lower-cased, dashed ("valheim-bepinex")
egg             the egg's path in the eggs checkout; resolve.py keys its
                memory of earlier runs on this
name            the egg's name without "server", "dedicated server",
                "(wine)", "(proton)"
description     the egg's
app             the dedicated server's Steam app id (SRCDS_APPID, APP_ID,
                APPID or STEAM_APPID); entries without one are left out
game            the game's own Steam app id, from Steam's store search, or
                null when the search found nothing: what the cover is of
cover           true when covers/<game>.jpg was saved
wine            the egg's docker image says wine or proton
startup         the egg's startup line, {{VAR}} placeholders as they are
ready           the line on the console that means the server is up
stop            what stops it ("^C" unless the egg says)
files           the egg's config.files rewrites, as they are
install         the egg's install script, as it is
settings[]      every egg variable that is not a port:
  var, label, default, help
  kind          text, number, bool, password (any var with PASSWORD or PASSWD)
  editable      the egg's user_editable, and not in HIDE
  choices       from an "in:a,b,c" rule
  ours          name, password, players or world, for the four we show first
ports           every variable matching PORT$|^PORT_|_PORT_, and SERVER_PORT
port_defaults   the egg's defaults for them: a note, the manager hands ports
                from its pool
fixed_ports     {VAR: port} a game insists on (FIXED_PORTS, curated)
implicit_ports  {VAR: offset} a game opens at SERVER_PORT + offset with no
                variable of its own (IMPLICIT_PORTS, curated)
memory_mib      what its guest gets: 6144, 8192 under wine, or MEMORY's
disk_bytes      the install's size from Steam (sizes.py); absent where Steam
                did not say (4 of 111)
```

[games.md](games.md#the-catalogue) has the reasoning behind each of these:
why memory is a class and not a measurement, which games have fixed or
implicit ports, and what a game takes of a box.

## Who reads it

```
servers/games              the manager: DD_GAMES_CATALOGUE (the games, by id) and
                           DD_GAMES_COVERS (GET /cover/<id> sends covers/<game>.jpg)
nix/modules/games/games.nix
                           dd.games.catalogue and dd.games.covers default to these
                           files; dd.games.fixedPorts is every fixed port and its
                           implicit ones, from the repo's catalogue
nix/modules/box/ports.nix  reserves those ports on a games box, beside the pool
nix/modules/games/needs.nix
                           one needs variant per game: memory_mib, 4 threads, and
                           disk from disk_bytes x 1.5 + 1 GiB (at least 2G; 15G
                           without disk_bytes)
nix/modules/games/game-run.py
                           the recipe inside the guest: install, files filled
                           from the settings, startup, ready, stop
```

The manager reads the file when it starts and refuses to start on one it
cannot parse. Its `Game` type
([servers/games/src/lib.rs](../../servers/games/src/lib.rs)) needs `id`,
`name`, `app`, `startup`, `install` and `memory_mib`; the rest have
defaults, and fields it does not know (`egg`, `disk_bytes`) are ignored.

`games.nix` reads the repository's catalogue directly, not
`dd.games.catalogue`, for the fixed ports: a test's catalogue is a
derivation, and reading one at evaluation would be import from
derivation. A box with another catalogue still reserves the repository's
fixed ports.

## Making it again

When the eggs should move, or a curated table in resolve.py changes:

1. Check out the eggs at the commit you want, with nothing changed:

   ```
   git clone https://github.com/pelican-eggs/games-steamcmd eggs
   git -C eggs checkout <commit>
   ```

2. Resolve, then size. Both write `catalogue.json` in place:

   ```
   data/games/resolve.py eggs data/games/catalogue.json data/games/covers/
   data/games/sizes.py   data/games/catalogue.json
   ```

   resolve.py prints how many games, covers and Wine servers it found;
   sizes.py how many it sized.

3. Read the diff. New games, games gone, a changed recipe, a changed
   memory class and new covers are all in it. Commit the catalogue and
   any new covers together.

resolve.py asks Steam's store search only for entries it has not seen:
it reads the catalogue already there and keeps each egg's `game`. A name
Steam did not know is not asked again; delete that entry from the file to
retry it. It sleeps between searches, and 20 seconds after a failed one.

## Checks

- **Style** ([.forgejo/checks/style.py](../../.forgejo/checks/style.py)):
  a changed `.json` must parse, a changed `.py` must pass `ruff format
  --check`, and any file over 1 MiB is refused.
- **Lint** ([.forgejo/checks/lint.py](../../.forgejo/checks/lint.py)):
  `ruff check` on a changed script; syntax errors and undefined names
  block, the rest warn.
- **Evaluation**: every box's system reads the catalogue through
  `games.nix` (the fixed ports), so a file that does not parse fails
  `checks.boxes` and every host build. `.#needs` reads every entry's
  `memory_mib` for its variant. Neither checks the recipe: a broken
  `startup` is found when someone starts that game.
- **The `games` VM test** ([nix/tests/games.nix](../../nix/tests/games.nix))
  runs a stand-in catalogue of one game ("probe", a Python web server), not
  this one. No test installs a real game from the catalogue.

A change under `data/` turns on CI's nix and VM parts
([ci.md](ci.md#how-a-run-fits-together)).

## Changing it

- **A game's memory** is an entry in `MEMORY` in resolve.py, by id, and
  a run of resolve.py. Editing `memory_mib` in the JSON by hand is undone
  by the next run.
- **A fixed port** is `FIXED_PORTS` in resolve.py. **An implicit port** is
  `IMPLICIT_PORTS`. Both are curated: add one only for a game known to do
  it. A fixed port is reserved on every games box and means one server of
  that game per box ([games.md](games.md#ports-and-the-firewall)).
- **A setting we name** (shown first on the card) is `OURS`; one kept
  out of sight is `HIDE`; one relabelled is `RELABEL`.
- **A new kind of data** gets its own folder under `data/`, its script
  beside it with the command that makes it in its header, and a line in
  the layout above.

## Pitfalls

- **resolve.py drops `disk_bytes`.** It rebuilds each entry from the egg
  and does not carry `disk_bytes` over, so run sizes.py after it every
  time. Without it, placement sizes every game at 15G.
- **`FIXED_PORTS` is keyed by the lower-cased name**, `IMPLICIT_PORTS` by
  the id. For a one-word name they are the same; for any other, a
  `FIXED_PORTS` key has to be the name with its spaces, not the dashed id.
- **One entry per id.** Two eggs that resolve to one id keep the Linux one
  over the Wine one, else the first in path order. A mod and its plain
  game have different names, so both stay.
- **Covers live in git.** About 50 KiB each, 4.8 MB together. A new egg
  adds one; nothing removes the cover of a game that left the catalogue.
