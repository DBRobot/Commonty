{ config, lib, ... }:
let
  base = config.dd.domain;
in
{
  # One wildcard cert for every subdomain and the bare name. DNS-01 needs no
  # inbound ports, which is why nothing here opens 80/443 - tailscale0 is
  # already trusted. The challenge writes a TXT record at Cloudflare with the
  # same token the front door uses.
  security.acme = {
    acceptTerms = true;
    # The role account, not a personal address. This one genuinely cannot come
    # from sops: the module passes it to lego as --email at eval time, so any
    # value here is a value in the store and in this public repo. Changing it
    # re-registers the acme account (the address is hashed into the account
    # directory); issued certs are unaffected.
    defaults.email = "distributed.datacenter@gmail.com";
    certs.${base} = {
      domain = "*.${base}";
      extraDomainNames = [ base ];
      dnsProvider = "cloudflare";
      environmentFile = config.sops.templates."cloudflare.env".path;
      # a group rather than the nginx user: kanidm once read these too, and
      # the next service that terminates its own tls will again
      group = "acmecerts";
    };
  };

  users.groups.acmecerts = { };
  users.users.nginx.extraGroups = [ "acmecerts" ];

  services.nginx = {
    enable = true;
    recommendedProxySettings = true;
    # the chat page alone is a 5M script; a phone on the tailnet felt it
    recommendedGzipSettings = true;
    # A name nothing here serves (a retired one, or a guess) used to land on
    # whichever vhost nginx listed first. Now it gets no tls handshake at all.
    virtualHosts."_" = {
      default = true;
      rejectSSL = true;
      locations."/".return = "444";
    };
    # The access log keeps the path and never the query string: that is
    # where share links, one-time codes and search terms travel. The
    # referrer, which is another page's whole address, is cut the same way.
    appendHttpConfig = ''
      map $http_referer $dd_referer_path {
        default "";
        "~^(?<dd_ref>[^?#]*)" $dd_ref;
      }
      log_format dd_path '$remote_addr - $remote_user [$time_local] '
                         '"$request_method $uri $server_protocol" $status $body_bytes_sent '
                         '"$dd_referer_path" "$http_user_agent"';
      access_log /var/log/nginx/access.log dd_path;
    '';
  };
  # and it is kept three weeks, not the half year logrotate would
  services.logrotate.settings.nginx.rotate = lib.mkForce 3;
}
