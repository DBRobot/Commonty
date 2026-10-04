# What a house's ad blocking needs of a box (types: nix/lib/needs-type.nix)
{
  pihole = {
    what = "ad and tracker blocking for a household's own devices, lookups sent out encrypted";
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
        inbound = "lan"; # never the Commonty network
      };
      depends = {
        dd-verify.local = true; # holds its password
        unbound.local = true;
      };
      privacy = "plaintext"; # every name the household's devices look up
    };
  };
}
