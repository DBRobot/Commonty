box.wait_for_unit("multi-user.target")

with subtest("the hash reached /etc/shadow"):
    box.succeed("getent shadow admin | cut -d: -f2 | grep -q '^[$]6[$]'")

with subtest("a wrong passphrase is refused at the keyboard"):
    box.wait_until_tty_matches("1", "login:")
    box.send_chars("admin\n")
    box.wait_until_tty_matches("1", "Password:")
    box.send_chars("not-the-passphrase\n")
    box.wait_until_tty_matches("1", "[Ll]ogin incorrect")

with subtest("the passphrase logs you in and wheel still works"):
    box.wait_until_tty_matches("1", "login:")
    box.send_chars("admin\n")
    box.wait_until_tty_matches("1", "Password:")
    box.send_chars("console-test-passphrase\n")
    box.wait_until_tty_matches("1", "admin@box")
    box.send_chars("sudo id -u > /tmp/who\n")
    # the shell makes the file before sudo writes to it: wait for what it says
    box.wait_until_succeeds("grep -qx 0 /tmp/who")
