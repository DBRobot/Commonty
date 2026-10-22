import json

from waits import wait_unit_active

box.wait_for_unit("multi-user.target")

# before: nothing is placed, everything runs
for u in [
    "dd-games",
    "gitea-runner-box",
    "gitea-runner-members-box",
    "thanos-store",
    "dd-snapshot-trim",
]:
    wait_unit_active(box, f"{u}.service")
box.succeed("systemctl is-active tangd.socket games-access.timer")
wait_unit_active(box, "pihole-ftl.service")
box.succeed("systemctl start dd-game@one.service")

# the release that places them: what ran stops, and the switch succeeds
box.succeed(
    "/run/current-system/specialisation/placed/bin/switch-to-configuration test 2>&1 | tail -3"
)
for u in [
    "dd-games",
    "dd-game@one",
    "gitea-runner-box",
    "gitea-runner-members-box",
    "thanos-store",
]:
    box.fail(f"systemctl is-active --quiet {u}.service")
box.fail("systemctl is-active --quiet pihole-ftl.service")
box.fail("systemctl is-active --quiet games-access.timer")
# a socket the switch leaves as it was (switch-to-configuration does not
# touch a changed socket); dd-control's first round stops it where the
# plan does not give its service (placed_test.go), and so does this
box.succeed("systemctl is-active tangd.socket")
box.succeed("systemctl stop tangd.socket")
# what every box runs is not touched
box.succeed("systemctl is-active dd-snapshot-trim.service")

carries = json.loads(box.succeed("cat /etc/dd-placed/carries.json"))
starts = json.loads(box.succeed("cat /etc/dd-placed/starts.json"))
print(json.dumps(carries, indent=1), json.dumps(starts, indent=1))
# what it carries: what the release gives it; not pihole or tang, which
# it is only built with
assert set(carries) == {
    "dd-games",
    "forgejo-runner",
    "members-runner",
    "thanos-store",
}, carries
assert set(carries["dd-games"]) == {
    "dd-games.service",
    "dd-game@*.service",
    "games-access.service",
    "games-access.timer",
}, carries["dd-games"]
assert carries["forgejo-runner"] == ["gitea-runner-box.service"], carries
assert carries["members-runner"] == ["gitea-runner-members-box.service"], carries
# a timer starts, not the run it starts; no template's instance
assert set(starts["dd-games"]) == {"dd-games.service", "games-access.timer"}, starts[
    "dd-games"
]

# built in and not carried, never started: a socket either
box.succeed("systemctl start tangd.socket pihole-ftl.service")
box.fail("systemctl is-active --quiet tangd.socket")
box.fail("systemctl is-active --quiet pihole-ftl.service")


def up(svc):
    box.succeed(f"touch /run/dd-placed/{svc}")
    box.succeed("systemctl start --no-block " + " ".join(f"'{u}'" for u in starts[svc]))


def down(svc):
    box.succeed("systemctl stop " + " ".join(f"'{u}'" for u in carries[svc]))
    box.succeed(f"rm /run/dd-placed/{svc}")


# a flag up: all of it runs, an instance too
up("dd-games")
wait_unit_active(box, "dd-games.service")
box.succeed("systemctl is-active games-access.timer")
box.succeed("systemctl start dd-game@one.service")
box.succeed("systemctl is-active dd-game@one.service")
# one runner's flag is that runner's alone
up("members-runner")
wait_unit_active(box, "gitea-runner-members-box.service")
box.succeed("systemctl start gitea-runner-box.service")
box.fail("systemctl is-active --quiet gitea-runner-box.service")

# down: every unit of it stops, the instance by its pattern, and none
# starts again without the flag
down("dd-games")
for u in ["dd-games.service", "dd-game@one.service", "games-access.timer"]:
    box.fail(f"systemctl is-active --quiet {u}")
box.succeed("systemctl start dd-games.service dd-game@two.service games-access.timer")
for u in ["dd-games.service", "dd-game@two.service", "games-access.timer"]:
    box.fail(f"systemctl is-active --quiet {u}")
box.succeed("systemctl is-active gitea-runner-members-box.service")
