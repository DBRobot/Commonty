// A repository's place in the forge's API, from an address's own parts.
// Kept apart from the pages' DOM so checks.git-path runs it under node.

// "owner/name" for /repos/<owner>/<name>: each part encoded, and none for
// a part that is no name ('.', '..', empty), which a browser would fold
// into the path above it
export function repoPath(owner, name) {
  if (![owner, name].every((p) => typeof p === 'string' && p && p !== '.' && p !== '..')) return null;
  return `${encodeURIComponent(owner)}/${encodeURIComponent(name)}`;
}

// a path in a repository, each part encoded; a '.' or '..' part (git keeps
// none) is sent as its own name, so a browser cannot fold it into the path
// above, and the forge finds nothing
export const enc = (path) => path.split('/').map((p) => (p === '.' || p === '..' ? encodeURIComponent(encodeURIComponent(p).replace(/\./g, '%2E')) : encodeURIComponent(p))).join('/');

// whether the repository the forge answered is the one the address named
export const sameRepo = (full, owner, name) => typeof full === 'string' && full.toLowerCase() === `${owner}/${name}`.toLowerCase();
