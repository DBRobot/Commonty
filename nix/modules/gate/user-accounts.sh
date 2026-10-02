#!/usr/bin/env bash
# An archive folder for every name in the directory, and a denied one.
set -euo pipefail

install -d -m 0755 -o root -g root $IMAGES
# where modules/webdav-media.nix sends a request whose token carried no
# usable username. Root-owned and unwritable on purpose.
install -d -m 0555 -o root -g root $IMAGES/__denied__

for f in $DIRECTORY/*.json; do
  [ -e "$f" ] || continue
  u=$(basename "$f" .json)
  case "$u" in *.passkeys) continue ;; esac
  # the same whitelist the verifier applies to a name
  printf '%s' "$u" | grep -qE '^[a-z0-9][a-z0-9._-]{0,63}$' || continue

  # 1000000 + 30 bits of sha256: numbers no distro hands out, no two
  # names collide in practice, and any box reaches the same one
  uid=$(( 1000000 + 0x$(printf 'dd-uid:%s' "$u" | sha256sum | cut -c1-8) % 1073741824 ))

  # 0750 rather than 0700 is load-bearing: on posix acls the group bits
  # are the mask that caps named entries, so 0700 masks the nginx
  # entry down to nothing. The group holds only this user, so the bits
  # grant no one anything by themselves.
  install -d -m 0750 -o "$uid" -g "$uid" $IMAGES/"$u"
  # a directory made under an earlier scheme (kanidm handed out the
  # numbers once) is taken over rather than orphaned
  [ "$(stat -c %u $IMAGES/"$u")" = "$uid" ] || chown -R "$uid:$uid" $IMAGES/"$u"

  # Nothing in here is readable by anyone but the uploader anyway - it
  # arrives encrypted.
  setfacl -m u:nginx:rwx $IMAGES/"$u"
  setfacl -d -m u:nginx:rwx $IMAGES/"$u"
  setfacl -d -m "u:$uid:rwx" $IMAGES/"$u"

  echo "user $u -> uid $uid"
done
