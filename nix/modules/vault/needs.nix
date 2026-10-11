# What Passwords needs of a box (types: nix/lib/needs-type.nix)
{
  vaultwarden = {
    what = "passwords: everyone's vault, encrypted on their own devices";
    # it serves the page master passwords are typed into
    trust = "owner";
    alwaysOn = true;
    grows = [ "passwords" ];
    # one vault: its records are one database
    instances = 1;
    units = [
      "dd-vault-end"
      "dd-usage-vaultwarden"
      "dd-vault-admin-token"
      "dd-forget-passwords" # its share of deleting whoever left (docs/dev/leaving.md)
      "dd-announce" # mail to members (modules/member-mail.nix) comes with the vault
    ];
    servesCode = true; # the web vault's page, where master passwords are typed
    base = {
      # its own files and room for a great many vaults' records: its
      # database is postgresql's, its attachments the storage pool's
      requires.disk.size.min = "10G";
      # node1, 2026-10-04
      uses = {
        memory = "20M";
        storage = "1M";
      };
      network = {
        outbound = [ "mail" ]; # mail.<domain>, the mail Worker, for notices
        inbound = "network";
      };
      depends = {
        postgresql.local = true;
        garage.local = false;
        nginx.local = true;
        # sign-in, and /run/dd-ended. The issuer is home.<domain>'s gate
        # and sessions are per box, so this box is the gateway's
        # (fleet/roles.json: with gateway)
        dd-verify.local = true;
        pwned-list.local = true; # breach checks never leave the box
      };
      privacy = "metadata"; # vaults are ciphertext; it sees names, emails, when things change
    };
  };

  vaultwarden-send = {
    what = "Send links: one thing shared with anyone, opened without an account";
    # it serves the page a Send is decrypted in
    trust = "owner";
    alwaysOn = true;
    servesCode = true; # the Send page decrypts in the visitor's browser
    base = {
      uses = {
        memory = "0";
        storage = "0";
      }; # vaultwarden's own process
      network = {
        outbound = "none";
        inbound = "public"; # only its own routes; the vault stays tailnet-only
      };
      depends = {
        vaultwarden.local = true;
        nginx.local = true;
      };
      privacy = "none"; # ciphertext, with the key in the link's #, which never reaches a server
    };
  };

  pwned-list = {
    what = "the breach list, so checking a password never leaves the box";
    # the public breach list, about 90G fetched on a timer
    traffic = {
      class = "high";
      downMbps = 100;
    };
    units = [
      "dd-pwned"
      "dd-pwned-first"
    ];
    base = {
      # 80G a copy, and a refresh builds the next beside it
      requires.disk.size.min = "170G";
      # node1, 2026-10-04: a whole fetch, curl with 64 transfers at once,
      # 6 hours, 77G in
      uses = {
        memory = "5.3G";
        storage = "80G";
      };
      network = {
        outbound = [ "api.pwnedpasswords.com" ];
        inbound = "localhost";
      };
      depends.nginx.local = true;
      privacy = "none"; # a public list
    };
  };
}
