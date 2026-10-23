// The Git pages' early ask (servers/forge/web/git-path.js), under node by
// checks.git-path: an address's parts never step out of /repos/<owner>/<name>.

import { repoPath, sameRepo, enc } from './git-path.mjs';
import assert from 'node:assert/strict';

assert.equal(repoPath('david', 'commonty'), 'david/commonty');
assert.equal(repoPath('a b', 'c?d#e'), 'a%20b/c%3Fd%23e');
assert.equal(repoPath('x', 'a/b'), 'x/a%2Fb');
assert.equal(repoPath('%2e%2e', 'y'), '%252e%252e/y');
for (const bad of [['..', 'admin'], ['.', 'x'], ['x', '..'], ['', 'x'], ['x', ''], [undefined, 'x']]) {
  assert.equal(repoPath(...bad), null, JSON.stringify(bad));
}
// what a browser makes of the path: it stays under /repos/<owner>/
for (const [o, n] of [['david', '..%2F..%2Fadmin'], ['david', 'a/../../b'], ['a', '\\..\\x']]) {
  const p = repoPath(o, n);
  const u = new URL(`/api/v1/repos/${p}/contents`, 'https://git.example');
  assert.ok(u.pathname.startsWith(`/api/v1/repos/${encodeURIComponent(o)}/`), u.pathname);
  assert.equal(u.pathname.split('/').length, 7, u.pathname);
}
// a path in the repository stays under its contents
assert.equal(enc('src/lib.rs'), 'src/lib.rs');
assert.equal(enc('a b/c#d'), 'a%20b/c%23d');
for (const path of ['../../admin', 'a/../../..', './x', '..', 'a/%2e%2e/b']) {
  const u = new URL(`/api/v1/repos/o/r/contents/${enc(path)}`, 'https://git.example');
  assert.ok(u.pathname.startsWith('/api/v1/repos/o/r/contents/'), `${path} -> ${u.pathname}`);
  assert.equal(u.pathname.split('/').length, 8 + path.split('/').length - 1, `${path} -> ${u.pathname}`);
}
assert.ok(sameRepo('David/Commonty', 'david', 'commonty'));
assert.ok(!sameRepo('david/commonty', 'david', 'other'));
assert.ok(!sameRepo('david/commonty-x', 'david', 'commonty'));
assert.ok(!sameRepo(undefined, 'david', 'commonty'));
console.log('git-path: ok');
