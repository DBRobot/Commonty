// id.<domain>: sign-in, once for every box (identity::session). No box
// serves this page: whichever box served it could ask the passkey for a
// sign-in good on every other. It is static: the page, its script and the
// site's styles. It holds no secret and stores nothing; a sign-in is kept
// in the browser (id.js), and each box checks it for itself.

import page from './id.html';
import script from './id.js';
import homeCss from './home.css';
import barCss from './bar.css';
import favicon from './favicon.svg';
import sans from './public-sans.woff2';
import mono400 from './plex-mono-400.woff2';
import mono500 from './plex-mono-500.woff2';

const STATIC = {
  '/_dd/static/id.js': [script, 'text/javascript; charset=utf-8'],
  '/_dd/static/home.css': [homeCss, 'text/css; charset=utf-8'],
  '/_dd/static/bar.css': [barCss, 'text/css; charset=utf-8'],
  '/_dd/static/favicon.svg': [favicon, 'image/svg+xml'],
  '/_dd/static/public-sans.woff2': [sans, 'font/woff2'],
  '/_dd/static/plex-mono-400.woff2': [mono400, 'font/woff2'],
  '/_dd/static/plex-mono-500.woff2': [mono500, 'font/woff2'],
};
const PAGES = new Set(['/', '/go', '/out']);

function headers(domain, type, cache) {
  return {
    'content-type': type,
    'cache-control': cache,
    'x-content-type-options': 'nosniff',
    // the origin alone: what a box checks the hand-off came from. No
    // referrer at all would post it as "null"
    'referrer-policy': 'strict-origin',
    'content-security-policy': [
      "default-src 'none'",
      "script-src 'self'",
      "style-src 'self'",
      "font-src 'self'",
      "img-src 'self' data:",
      // the passkeys on file, from the box that sent the browser here
      `connect-src https://*.${domain}`,
      // and the hand-off, posted to that box
      `form-action https://*.${domain} https://${domain}`,
      "frame-ancestors 'none'",
      "base-uri 'none'",
    ].join('; '),
  };
}

export default {
  async fetch(req, env) {
    // a body is read before any answer: one left unread faults the next request
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      await req.text().catch(() => '');
      return new Response('not here', { status: 405 });
    }
    const path = new URL(req.url).pathname;
    if (STATIC[path]) {
      const [body, type] = STATIC[path];
      return new Response(body, { headers: headers(env.DOMAIN, type, 'public, max-age=300') });
    }
    if (PAGES.has(path)) return new Response(page, { headers: headers(env.DOMAIN, 'text/html; charset=utf-8', 'no-store') });
    return new Response('not here', { status: 404, headers: headers(env.DOMAIN, 'text/plain', 'no-store') });
  },
};
