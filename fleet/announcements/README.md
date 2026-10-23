# Announcements

A change to the fleet's policies, one file each, written by
`dd policy announce` (client/cli/src/policy.rs) and shipped in the signed
release. The box that writes to members mails each one to every member it
names, once, and refuses one under 30 days' notice
(nix/modules/member-mail.nix). Nobody logs in to a box to send it.
