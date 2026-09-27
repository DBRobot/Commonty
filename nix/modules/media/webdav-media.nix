{ config, ... }:
let
  base = config.dd.domain;
  host = "files.${base}";
  images = "/srv/images";

  # The dav block, on the one root that is left: /srv/images, per-user
  # archives that arrive already encrypted (rclone crypt on the client),
  # so nothing on this host can read them and nothing gets an acl. The
  # $dav_dir whitelist below keeps a request inside its own name's
  # directory. See modules/gate/user-accounts.nix for the directories.
  dav = dir: listing: ''
    auth_request /_dd/verify;
    auth_request_set $dav_user $upstream_http_x_auth_request_preferred_username;
    # $dav_dir is $dav_user filtered through the map below, so a missing
    # or malformed header cannot name a directory
    alias ${dir}/$dav_dir/;

    dav_methods PUT DELETE MKCOL COPY MOVE;
    dav_ext_methods PROPFIND OPTIONS;
    dav_access user:rw group:rw;
    create_full_put_path on;

    client_max_body_size 0; # movies, disk images
    client_body_timeout 600s;
    send_timeout 600s;
    autoindex on;
    # html for a person in a browser; json for `dd image`, which lists a
    # restic repository's type directories this way rather than parsing
    # PROPFIND xml
    autoindex_format ${listing};
  '';
in
{
  # /srv/users is retired. A member's files are their library now: named
  # and encrypted on their own machine, stored as ciphertext under
  # /_dd/dav (modules/gate/verify.nix, box/library/src/dav.rs), opened in
  # the browser at /_dd/files or mounted with `dd media`. This host keeps
  # /images/, which is the same idea by hand: archives that arrive
  # already encrypted by rclone crypt on the client, for `dd image`.
  #
  # Nothing under /srv/users is deleted here. It is still backed up and
  # still on the disk; it is only no longer served. Removing it is a
  # decision for whoever owns the box, once they have looked.
  services.nginx.virtualHosts.${host} = {
    useACMEHost = base;
    forceSSL = true;

    # the door a person lands on: their library, not a folder on this box
    locations."/".extraConfig = ''
      return 302 https://files.${base}/_dd/files;
    '';
    # Verified with rclone crypt -> chunker -> webdav: an unknown-size stream
    # (`dd if=/dev/sdX | zstd | rclone rcat`) arrives as fixed-size chunk PUTs,
    # so no scratch copy of the image is ever needed on the client.
    locations."/images/".extraConfig = dav images "json";

  };

  # An empty $dav_user would make the alias /srv/images/ - the root itself,
  # shared by everyone - and create_full_put_path would happily mkdir in it.
  # Kanidm will not issue a name with a slash in it, but the directory name
  # here comes from a header, so it gets whitelisted rather than trusted.
  # This is a map and not `if ($dav_user = "")` because if runs in the rewrite
  # phase, before auth_request has set the variable: the test would always see
  # an empty string. Map variables are evaluated where they are used.
  services.nginx.appendHttpConfig = ''

    map $dav_user $dav_dir {
      # __denied__ is a real 0555 root-owned directory created by
      # user-accounts in modules/user-accounts.nix. It exists so a request that
      # got past auth with no usable username lands somewhere provably
      # unwritable instead of somewhere nginx might create. The response is
      # still a 500, not a 403: nginx's dav module maps the EACCES from
      # open() to 500 and no permission arrangement changes that. Masking it
      # with error_page would also swallow real server errors here, so it is
      # left honest and ugly.
      default            "__denied__";
      "~^[a-zA-Z0-9._-]+$" $dav_user;
    }
  '';

  # nginx's unit is sandboxed with a read-only /srv, so every PUT failed
  # with "mkdir() ... (30: Read-only file system)" despite correct auth.
  systemd.services.nginx.serviceConfig.ReadWritePaths = [ images ];

  # nginx writes the upload here before moving it into place; without a
  # temp path on the same filesystem every PUT is a cross-device copy
  systemd.tmpfiles.rules = [ "d /srv/upload-tmp 0700 nginx nginx -" ];

}
