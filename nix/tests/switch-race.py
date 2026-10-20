from waits import wait_socket, wait_unit_active

SWITCHES = 20

box.wait_for_unit("multi-user.target")
wait_socket(box, 22)

box.succeed("systemctl start churn.service")
wait_unit_active(box, "churn.service")
box.wait_until_succeeds("test $(cat /run/churn-logins 2>/dev/null || echo 0) -ge 3")
logins_before = int(box.succeed("cat /run/churn-logins"))
# admin's systemd as the switches find it: lingering, the one from boot
manager = box.succeed("systemctl show -P InvocationID user@1000.service").strip()

failed = []
for i in range(SWITCHES):
    code, out = box.execute("/run/current-system/bin/switch-to-configuration test 2>&1")
    if code != 0:
        failed.append(f"switch {i}: status {code}\n{out}")

logins = int(box.succeed("cat /run/churn-logins")) - logins_before
box.succeed("systemctl stop churn.service")
print(
    f"{SWITCHES} switches, {len(failed)} failed, {logins} logins as admin during them"
)
assert logins >= SWITCHES, f"only {logins} logins during {SWITCHES} switches: no race"
assert not failed, "\n\n".join(failed)
# and the logins never stopped or restarted it
now = box.succeed("systemctl show -P InvocationID user@1000.service").strip()
assert manager and now == manager, f"admin's systemd restarted ({manager} -> {now})"
