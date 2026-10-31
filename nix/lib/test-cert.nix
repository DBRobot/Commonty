# A test's self-signed certificate, the same bytes on every build. One with
# a fresh key each build broke the tests trusting it (2026-10-27): the trust
# bundle made from it is kept, the cert was collected and built again with
# a new key, and nothing trusted the forge's name any more.
{ pkgs }:
{
  subject,
  names,
  # addresses it is for too, for a client that dials one by address
  ips ? [ ],
  name ? "test-cert",
}:
let
  q = pkgs.lib.escapeShellArg;
in
pkgs.runCommand name
  {
    nativeBuildInputs = [
      pkgs.openssl
      pkgs.xxd
    ];
  }
  ''
    mkdir $out
    # P-256 key (SEC1) whose scalar is a hash of the subject; signed per
    # RFC 6979, fixed serial and dates
    d=$(printf %s ${q "commonty test cert ${subject}"} | sha256sum | cut -c1-64)
    printf '30310201010420%sa00a06082a8648ce3d030107' "$d" | xxd -r -p \
      | openssl ec -inform der -out $out/key.pem
    openssl req -x509 -new -key $out/key.pem -sha256 -sigopt nonce-type:1 -set_serial 1 \
      -not_before 20260101000000Z -not_after 20360101000000Z \
      -subj ${q "/CN=${subject}"} \
      -addext ${q "subjectAltName=${pkgs.lib.concatStringsSep "," (map (n: "DNS:${n}") names ++ map (i: "IP:${i}") ips)}"} \
      -out $out/cert.pem
  ''
