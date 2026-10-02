{ config, ... }:
{
  # People's files and the Movies & TV and Files tiles. Media lives
  # encrypted in each person's library and plays on the page's own player
  # (box/media, box/transcode); the demo's is the demo library.
  imports = [
    ./_sops.nix
    ../modules/gate/user-accounts.nix
    ../modules/media/webdav-media.nix
  ];

  dd.home.services = [
    {
      name = "Movies & TV";
      blurb = "Films and shows from your own library.";
      # a member's films are their own library, opened by their passkey;
      # the demo's are the demo library (modules/library/libraries.nix),
      # on the same page and the same player: it reads them and plays them,
      # and the gate lets it do nothing else (verify's demo_allows)
      url = "https://files.${config.dd.domain}/_dd/media";
      demo = "read";
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

  dd.backup.paths = [
    "/srv/images" # archives of old computers, already ciphertext
  ];
}
