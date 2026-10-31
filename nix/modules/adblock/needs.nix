# What a house's ad blocking needs of a box (types: nix/lib/needs-type.nix)
{
  pihole = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    what = "ad and tracker blocking for a household's own devices, and its owner's set to ask it on the Commonty network; lookups sent out encrypted";
    # a house's own DNS: on a box of that household, or the owner's
    trust = "household";
    alwaysOn = true;
    units = [
      "pihole-ftl"
      "dd-pihole-lists"
      # its run after boot, a switch, or the flag going up (box/remote-jobs.nix)
      "dd-pihole-lists-after-switch"
    ];
    perHouse = 1;
    # the house's devices ask it at this box's address (the router hands
    # that out): it stays while the box is up, and moves only when it is gone
    moves = "on-failure";
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
  tang = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    what = "opens a household's home controller's card at boot, by one blinded key operation, on the house network alone; never learns the card's key";
    # one blinded key operation, and requires.place keeps it in its house
    trust = "any";
    units = [ "tangd*" ];
    perHouse = 1;
    # its keys are this box's: a controller's card opens only with them, so
    # it stays while the box is up, and moves only when it is gone
    moves = "on-failure";
    base = {
      requires.place = "house";
      uses = {
        memory = "8M";
        storage = "1M";
        estimated = true;
      };
      network = {
        outbound = "none";
        inbound = "lan";
      };
      privacy = "none";
    };
  };
}
