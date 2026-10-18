// a message's id: when it came, in base 36, then randomness - sortable by
// time, the order the box lists them in
export function newId(now = Date.now()) {
  const r = crypto.getRandomValues(new Uint8Array(8));
  return `${now.toString(36).padStart(9, '0')}${[...r].map((b) => b.toString(36).padStart(2, '0')).join('').slice(0, 12)}`;
}
export const ID = /^[0-9a-z]{21}$/;
