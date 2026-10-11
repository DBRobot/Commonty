# The names a box serves over https: every nginx host on its certificate,
# and their aliases, under the fleet's domain. Its certificate is for these
# alone, and the DNS Worker writes a challenge for these alone, for it.
lib: config:
let
  base = config.dd.domain;
  ours = n: n == base || lib.hasSuffix ".${base}" n;
  hosts = lib.filterAttrs (_: v: v.useACMEHost == base) config.services.nginx.virtualHosts;
  # a host's name is its attribute's, unless it says otherwise
  named = lib.concatLists (
    lib.mapAttrsToList (
      n: v: [ (if v.serverName != null then v.serverName else n) ] ++ v.serverAliases
    ) hosts
  );
  names = lib.unique (lib.sort lib.lessThan (lib.filter ours named));
in
# the bare domain first, as it always has been the certificate's own name
lib.optional (lib.elem base names) base ++ lib.remove base names
