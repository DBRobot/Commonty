// While a service moves to another box its names answer 503 with
// Retry-After (docs/dev/rollout.md). Ask again on that rhythm, and load
// the page the member came for as soon as the answer is anything else.
(() => {
  const again = async () => {
    let wait = 3;
    try {
      const r = await fetch(location.href, { method: "HEAD", cache: "no-store" });
      if (r.status !== 503) {
        location.reload();
        return;
      }
      wait = Math.min(Math.max(Number(r.headers.get("Retry-After")) || 3, 1), 10);
    } catch {
      // not answering at all yet: the same rhythm
    }
    setTimeout(again, wait * 1000);
  };
  setTimeout(again, 2000);
})();
