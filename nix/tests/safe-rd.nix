# Where the sign-in pages send you afterwards (safeRd, servers/web-shared/
# webauthn.js): a path on this site, whatever the query string says. Each
# case is followed the way the browser would, and must stay on the site.
{ pkgs, ... }:
pkgs.runCommand "safe-rd" { nativeBuildInputs = [ pkgs.nodejs ]; } ''
  cp ${../../servers/web-shared/webauthn.js} webauthn.mjs
  cat > check.mjs <<'JS'
  globalThis.location = { origin: 'https://home.example.test', href: 'https://home.example.test/_dd/login' };
  const { safeRd } = await import('./webauthn.mjs');
  const off = [
    'https://evil.test/', '//evil.test', '/\\evil.test', '\\/evil.test', '/\t/evil.test',
    '/.//evil.test', '/./\\evil.test', '/a/..//evil.test', '/%2e//evil.test', 'javascript:alert(1)',
    ' //evil.test', '/..//evil.test',
  ];
  const kept = { '/_dd/files?x=1#y': '/_dd/files?x=1#y', '/': '/', "": '/', '/_dd/home': '/_dd/home' };
  let bad = 0;
  for (const raw of off) {
    const out = safeRd(raw);
    const host = new URL(out, location.href).host;
    if (host !== 'home.example.test' || !out.startsWith('/') || /^[/\\]{2}/.test(out)) { console.log('LEAVES', JSON.stringify(raw), '->', out); bad++; }
  }
  for (const [raw, want] of Object.entries(kept)) {
    if (safeRd(raw) !== want) { console.log('CHANGED', JSON.stringify(raw), '->', safeRd(raw)); bad++; }
  }
  console.log(bad ? `''${bad} wrong` : 'every rd stays on the site');
  process.exit(bad ? 1 : 0);
  JS
  node check.mjs
  touch $out
''
