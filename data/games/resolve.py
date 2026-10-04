#!/usr/bin/env python3
"""Turn the pelican eggs into the catalogue: every Steam dedicated server the
eggs know, with its game's store id (for the cover) and our names for its
common settings. Run when the eggs input moves; the result is committed.

  games/resolve.py <eggs checkout> games/catalogue.json games/covers/
"""

import json, glob, re, sys, os, time, urllib.request, urllib.parse

eggs_dir, out, covers = sys.argv[1:4]
os.makedirs(covers, exist_ok=True)

# the eggs' names for settings, and ours
OURS = {
    "SERVER_NAME": ("name", "Server name"),
    "SRV_NAME": ("name", "Server name"),
    "HOSTNAME": ("name", "Server name"),
    "SERVERNAME": ("name", "Server name"),
    "PASSWORD": ("password", "Password"),
    "SERVER_PASSWORD": ("password", "Password"),
    "SRV_PASSWORD": ("password", "Password"),
    "SERVER_PW": ("password", "Password"),
    "MAX_PLAYERS": ("players", "Players"),
    "MAXPLAYERS": ("players", "Players"),
    "SLOTS": ("players", "Players"),
    "SERVER_MAX_PLAYERS": ("players", "Players"),
    "WORLD": ("world", "World name"),
    "WORLD_NAME": ("world", "World name"),
    "MAP": ("world", "Map"),
    "SRCDS_MAP": ("world", "Map"),
}
# not ours by name, but relabelled and defaulted off: a restart should be quick
RELABEL = {"AUTO_UPDATE": ("Update on start", "0")}
# games that tell their clients a port of their own and ignore -Port for it:
# that number on the box, host and guest alike, and one of that game per box.
# Every other port the manager hands from the pool. Curated: an egg's default
# is what it would pick, not what it needs.
FIXED_PORTS = {"satisfactory": {"SERVER_PORT": 7777, "RELIABLE_PORT": 8888}}
HIDE = {
    "SRCDS_APPID",
    "SRCDS_BETAID",
    "SRCDS_BETAPASS",
    "VALIDATE",
    "LD_LIBRARY_PATH",
    "CONSOLE_FILTER",
    "STEAM_USER",
    "STEAM_PASS",
    "STEAM_AUTH",
    "WINETRICKS_RUN",
    "WINDOWS_INSTALL",
    "INSTALL_FLAGS",
    "ADDITIONAL_ARGS",
}
PORTS = re.compile(r"PORT$|^PORT_|_PORT_")


def store(term):
    url = (
        "https://store.steampowered.com/api/storesearch/?term=%s&l=english&cc=US"
        % urllib.parse.quote(term)
    )
    try:
        with urllib.request.urlopen(url, timeout=15) as r:
            items = json.load(r).get("items", [])
    except Exception:
        time.sleep(20)
        return None
    return items[0]["id"] if items else None


def cover(game_id):
    path = os.path.join(covers, "%d.jpg" % game_id)
    if os.path.exists(path):
        return True
    url = (
        "https://cdn.cloudflare.steamstatic.com/steam/apps/%d/library_600x900.jpg"
        % game_id
    )
    try:
        with urllib.request.urlopen(url, timeout=15) as r, open(path, "wb") as f:
            f.write(r.read())
        return True
    except Exception:
        return False


old = {}
if os.path.exists(out):
    old = {g["egg"]: g for g in json.load(open(out))["games"]}

games = []
for p in sorted(glob.glob(os.path.join(eggs_dir, "**", "egg-*.json"), recursive=True)):
    d = json.load(open(p))
    if "startup" not in d or "variables" not in d:
        continue
    vars_ = {v["env_variable"]: v for v in d["variables"]}
    app = next(
        (
            vars_[k]["default_value"]
            for k in vars_
            if k in ("SRCDS_APPID", "APP_ID", "APPID", "STEAM_APPID")
        ),
        None,
    )
    if not app or not str(app).isdigit():
        continue
    images = " ".join((d.get("docker_images") or {}).values()).lower()
    wine = "wine" in images or "proton" in images
    name = (
        re.sub(
            r"\s*[\(\[]?(wine|proton|dedicated\s*server|server)[\)\]]?\s*$",
            "",
            d["name"],
            flags=re.I,
        ).strip(" :-")
        or d["name"]
    )
    egg = os.path.relpath(p, eggs_dir)
    prev = old.get(egg)
    game_id = prev["game"] if prev and prev.get("game") else None
    # a name steam did not know last time is not asked again; delete the
    # entry from the catalogue to retry it
    if not game_id and not (prev and "game" in prev):
        term = re.sub(r"\b(dedicated|server|srcds|hlds)\b", "", name, flags=re.I).strip(
            " :-"
        )
        game_id = store(term) or store(name)
        time.sleep(2)
    has_cover = bool(game_id) and cover(game_id)
    settings = []
    for v in d["variables"]:
        k = v["env_variable"]
        if PORTS.search(k):
            continue
        hidden = k in HIDE
        rules = v.get("rules", "")
        rules = "|".join(rules) if isinstance(rules, list) else str(rules or "")
        kind = (
            "number"
            if "integer" in rules or "numeric" in rules
            else ("bool" if "boolean" in rules else "text")
        )
        if "PASSWORD" in k or "PASSWD" in k:
            kind = "password"
        m = re.search(
            r"(?:^|\|)in:([^|]+)", "|".join(r.strip() for r in rules.split("|"))
        )
        # a hidden one keeps its default in the env; a person never sees it
        entry = {
            "var": k,
            "label": v["name"],
            "default": v["default_value"],
            "kind": kind,
            "help": v.get("description", ""),
            "editable": bool(v.get("user_editable")) and not hidden,
        }
        if m:
            entry["choices"] = m.group(1).split(",")
        if k in RELABEL:
            entry["label"], entry["default"] = RELABEL[k]
            entry["kind"] = "bool"
        if k in OURS:
            ours, label = OURS[k]
            # an egg's PASSWORD may be for telnet or rcon: keep the egg's label then
            if ours == "password" and re.search(
                r"rcon|telnet|admin", v["name"] + " " + v.get("description", ""), re.I
            ):
                pass
            else:
                entry["ours"], entry["label"] = ours, label
        settings.append(entry)
    cfg = d.get("config", {})
    files = cfg.get("files")
    if isinstance(files, str):
        try:
            files = json.loads(files) if files.strip() else {}
        except Exception:
            files = {}
    ready = cfg.get("startup")
    if isinstance(ready, str):
        try:
            ready = json.loads(ready).get("done")
        except Exception:
            ready = None
    elif isinstance(ready, dict):
        ready = ready.get("done")
    if isinstance(ready, list):
        ready = ready[0] if ready else None
    stop = cfg.get("stop", "^C")
    games.append(
        {
            "id": re.sub(r"[^a-z0-9]+", "-", name.lower()).strip("-"),
            "egg": egg,
            "name": name,
            "description": d.get("description", ""),
            "app": int(app),
            "game": game_id,
            "cover": has_cover,
            "wine": wine,
            "startup": d["startup"],
            "ready": ready,
            "stop": stop,
            "files": files or {},
            "install": d["scripts"]["installation"]["script"],
            "settings": settings,
            "ports": sorted({k for k in vars_ if PORTS.search(k)} | {"SERVER_PORT"}),
            "port_defaults": {
                k: vars_[k]["default_value"] for k in vars_ if PORTS.search(k)
            },
            "fixed_ports": FIXED_PORTS.get(name.lower(), {}),
        }
    )
# one entry per game: prefer the linux one over the wine one, the plain over the mod
seen = {}
for g in games:
    key = g["id"]
    if key not in seen or (seen[key]["wine"] and not g["wine"]):
        seen[key] = g
games = sorted(seen.values(), key=lambda g: g["name"].lower())
json.dump(
    {"eggs": os.path.basename(os.path.abspath(eggs_dir)), "games": games},
    open(out, "w"),
    indent=1,
)
print(
    len(games),
    "games,",
    sum(1 for g in games if g["cover"]),
    "with covers,",
    sum(1 for g in games if g["wine"]),
    "wine",
)
