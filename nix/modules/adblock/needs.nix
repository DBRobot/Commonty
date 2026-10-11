# What a house's ad blocking needs of a box (types: nix/lib/needs-type.nix)
{
  pihole = {
    what = "ad and tracker blocking for a household's own devices, and its owner's set to ask it on the Commonty network; lookups sent out encrypted";
    units = [
      "pihole-ftl"
      "dd-pihole-lists"
    ];
    perHouse = 1;
    base = {
      # on the house's own network, with its household in box.json: only
      # a household with a box of its own has this
      requires.place = "house";
      # node1, 2026-10-04: pihole-ftl and unbound
      uses = {
        memory = "28M";
        storage = "7M";
      };
      network = {
        outbound = [
          "dns.quad9.net"
          "cloudflare-dns.com"
          "raw.githubusercontent.com"
        ];
        # the house network, and on the Commonty network the box's owner's
        # devices alone (net/owner.nix); the network hands everyone else Quad9
        inbound = "lan";
      };
      depends = {
        dd-verify.local = true; # holds its password
        unbound.local = true;
      };
      privacy = "plaintext"; # every name the household's devices look up
    };
  };
}
