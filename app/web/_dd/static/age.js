// What every sign-up asks before an account is made: month and year of
// birth (no day, nothing chosen for them), a parent's or guardian's
// agreement from 13 to 17, and "I agree to the Terms of use and the Privacy
// policy", unticked. Only the check happens, here: the birth date is never
// kept, sent or logged. Under 13 is refused, and the refusal is remembered
// on this device (`remembered`, `remember`), so answering again changes
// nothing. The same rule as `dd` (client/cli/src/consent.rs).
//
// The page holds the fields (see the join page or the app's sign-up):
//   #born-month, #born-year      the two selects
//   #guardian-row, #guardian     the parent's or guardian's box, hidden until 13-17
//   #agree                       the terms
//   #age-refused                 what under 13 is told
// and `consent` keeps `button` disabled until the answers allow it.

const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July',
  'August', 'September', 'October', 'November', 'December'];

/// 'under13', 'teen' or 'adult': whole months to this one, the birth month
/// counted as reached, since no day is asked
export function ageOf(year, month, now = new Date()) {
  const months = (now.getFullYear() - year) * 12 + (now.getMonth() + 1 - month);
  if (months < 13 * 12) return 'under13';
  if (months < 18 * 12) return 'teen';
  return 'adult';
}

const COOKIE = 'dd_age=refused';

/// the browser's memory of a refusal: its storage, and a cookie, so clearing
/// one is not enough
export const browserMemory = {
  async remembered() {
    let kept = false;
    try { kept = localStorage.getItem('dd_age') === 'refused'; } catch { /* storage off */ }
    return kept || document.cookie.split(';').some((c) => c.trim() === COOKIE);
  },
  async remember() {
    try { localStorage.setItem('dd_age', 'refused'); } catch { /* storage off */ }
    document.cookie = `${COOKIE}; path=/; max-age=${10 * 365 * 24 * 3600}; samesite=strict; secure`;
  },
};

/// Wire the questions to `button`. `memory` is where a refusal is kept
/// ({ remembered, remember }): the browser's, or the app's store.
/// Returns `ready()`: whether the answers let an account be made now.
export async function consent(button, memory = browserMemory) {
  const $ = (id) => document.getElementById(id);
  const month = $('born-month');
  const year = $('born-year');
  const guardian = $('guardian');
  const guardianRow = $('guardian-row');
  const agree = $('agree');
  const refused = $('age-refused');

  if (month.options.length <= 1) {
    for (const [i, m] of MONTHS.entries()) month.add(new Option(m, String(i + 1)));
  }
  if (year.options.length <= 1) {
    const now = new Date().getFullYear();
    for (let y = now; y >= now - 110; y--) year.add(new Option(String(y), String(y)));
  }

  let out = false;
  const shut = () => {
    out = true;
    for (const f of [month, year, guardian, agree]) f.disabled = true;
    guardianRow.hidden = true;
    refused.hidden = false;
    button.disabled = true;
  };

  const ready = () => {
    if (out || !month.value || !year.value || !agree.checked) return false;
    const age = ageOf(Number(year.value), Number(month.value));
    return age === 'adult' || (age === 'teen' && guardian.checked);
  };

  const update = async () => {
    if (out) return;
    if (month.value && year.value) {
      const age = ageOf(Number(year.value), Number(month.value));
      if (age === 'under13') {
        await memory.remember();
        shut();
        refused.focus();
        return;
      }
      guardianRow.hidden = age !== 'teen';
      if (age !== 'teen') guardian.checked = false;
    } else {
      guardianRow.hidden = true;
    }
    button.disabled = !ready();
  };

  for (const f of [month, year, guardian, agree]) f.addEventListener('change', update);
  button.disabled = true;
  if (await memory.remembered()) shut();
  else update();
  return ready;
}
