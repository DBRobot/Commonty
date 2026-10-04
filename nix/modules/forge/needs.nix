# What the forge needs of a box (types: nix/lib/needs-type.nix)
{
  forgejo = {
    what = "git: repositories, issues, pull requests, CI runs, releases";
    # its repositories are on one box's disk, beside its database
    instances = 1;
    base = {
      # repositories, LFS, release files of up to 100M each: overestimated
      requires.disk.size.min = "20G";
      # node1, 2026-10-04: peak; its dataset (and 70M in postgresql)
      uses = {
        memory = "151M";
        storage = "400M";
      };
      network = {
        outbound = [ "github.com" ]; # the actions CI checks out
        inbound = "tailnet"; # signing in and pushing
      };
      depends = {
        postgresql.local = true;
        nginx.local = true;
        dd-verify.local = true; # sign-in in front of every page
        sshd.local = true; # git over ssh is the box's own sshd
      };
      privacy = "plaintext"; # repositories are kept as they are pushed
    };
  };

  forgejo-public = {
    what = "the public repositories' pages, to anyone";
    base = {
      uses = {
        memory = "0";
        storage = "0";
      }; # forgejo's own process
      network = {
        outbound = "none";
        inbound = "public";
      };
      depends = {
        forgejo.local = true;
        nginx.local = true;
      };
      privacy = "none"; # only what was made public
    };
  };

  forgejo-mirror = {
    what = "pushes the fleet's chosen repositories to their GitHub copies (fleet/policy.json)";
    base = {
      uses = {
        memory = "64M";
        storage = "0";
        estimated = true; # a few git pushes, every eight hours and on push
      };
      network = {
        outbound = [ "github.com" ];
        inbound = "localhost";
      };
      depends.forgejo.local = true; # it reads the forge's socket
      privacy = "none";
    };
  };
}
