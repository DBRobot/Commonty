{ config, lib, ... }:
{
  # The public name every service hangs off. One place: the move from the
  # duckdns name to commonty.org was a change here, not an edit in eleven
  # files.
  options.dd.domain = lib.mkOption {
    type = lib.types.str;
    description = "Base domain; services are subdomains of it.";
  };
  # the fleet's repository on its forge, as owner/name: where releases are
  # read from, what ci protects, what the mirror pushes
  options.dd.repo = lib.mkOption {
    type = lib.types.str;
    description = "the fleet's repository on the forge, owner/name";
  };
  # The fleet's zone at Cloudflare is kept by the DNS Worker once `dd dns
  # setup` has written fleet/dns-worker.json: then no box holds a token for
  # it, certificates' challenges are asked of it (gate/acme.nix) and the
  # names follow the release (nix/lib/fleet-names.nix). Before that, the
  # gateway does both with its own token.
  options.dd.dnsWorker = lib.mkOption {
    type = lib.types.str;
    default =
      if builtins.pathExists ../../../fleet/dns-worker.json then
        "https://dns.${config.dd.domain}"
      else
        "";
    description = "the DNS Worker's address; empty: the gateway's own token keeps the zone";
  };
  # The domain the names of a box that joined on its own key are under: its
  # store's, and what the plan gives it (gate/acme.nix, storage/store.nix, the
  # DNS Worker's RECORDS joinedZone). One place, for them to be under another
  # domain than the fleet's own pages
  options.dd.joinedDomain = lib.mkOption {
    type = lib.types.str;
    default = config.dd.domain;
    description = "the domain a joined box's names are under";
  };
  # Sign-in, once for every box, at the id Worker (servers/system/id) once
  # `dd id setup` has written fleet/id-worker.json: each box's sessions are
  # then for each of its names alone (servers/gate/src/sso.rs)
  options.dd.idWorker = lib.mkOption {
    type = lib.types.str;
    default =
      if builtins.pathExists ../../../fleet/id-worker.json then "https://id.${config.dd.domain}" else "";
    description = "the id Worker's address; empty: each gate's own sign-in page";
  };
}
