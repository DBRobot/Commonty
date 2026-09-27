{ config, ... }:
{
  # The demo's media, people's files, the archive catalog. Members' media
  # is not here any more: it lives encrypted in their libraries and plays
  # on their devices (client/media, the app). Jellyfin stays for the demo,
  # over a plain folder of films that are nobody's.
  imports = [
    ./_sops.nix
    ../modules/media/jellyfin.nix
    ../modules/media/archive-catalog.nix
    ../modules/gate/user-accounts.nix
    ../modules/media/webdav-media.nix
  ];
  users.users.admin.extraGroups = [ "media" ]; # copy demo films into /srv/media without sudo

  dd.home.services = [
    {
      name = "Movies & TV";
      blurb = "Films and shows from your own library.";
      # a member's films are their own library, opened by their passkey
      url = "https://files.${config.dd.domain}/_dd/media";
      # the demo has no library and no passkey: it gets the box's own
      # films, through jellyfin, straight into the sso plugin because
      # jellyfin's own login page is for nobody here
      demoUrl = "https://jellyfin.${config.dd.domain}/sso/OID/p/dd";
      # a jellyfin user like any other, made on first visit, with
      # jellyfin's defaults: watch, no admin. jellyfin is the limit
      demo = "full";
      icon = "videos";
      color = "#b4457a";
      rank = 20;
    }
    {
      name = "Files";
      blurb = "Folders and documents, encrypted before they leave your device.";
      url = "https://files.${config.dd.domain}/_dd/files";
      # the demo has a library of its own (modules/library/libraries.nix),
      # so the tile opens: it reads that one, writes nothing anywhere, and
      # the gate is what enforces it rather than this line
      demo = "read";
      icon = "files";
      color = "#2f6fd6";
      rank = 30;
    }
  ];

  # jellyfin's state is the demo's and is not backed up: nothing in it is
  # anyone's
  dd.backup.paths = [
    "/srv/users" # people's uploads
    "/srv/images" # archives of old computers, already ciphertext
  ];
}
