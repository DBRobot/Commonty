# The tunnel's rules, for both ways of running cloudflared (modules/gate/public.nix).
# Every name in `outside` lands on nginx's tunnel listener; `ssh.<domain>`
# (only when sshPort is set) is the one name that lands on anything else, the
# forge's own sshd, and no other name is ever sent there. The last rule is the
# 404 that answers whatever matched nothing.
{
  lib,
  base,
  outside,
  tunnelPort,
  sshPort ? null,
}:
map (host: {
  hostname = host;
  service = "https://127.0.0.1:${toString tunnelPort}";
  originRequest.originServerName = host;
}) outside
++ lib.optional (sshPort != null) {
  hostname = "ssh.${base}";
  service = "ssh://127.0.0.1:${toString sshPort}";
}
++ [ { service = "http_status:404"; } ]
