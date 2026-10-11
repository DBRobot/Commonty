# UI guidelines

Every page Commonty shows - the site, the app's own screens, the Git, Docs
and Games pages - is held to three things, in this order when they pull
against each other:

1. **Fast.** A page is there the moment it is asked for. Not "loads
   quickly": draws at once, from what it already has, and fills in.
2. **Cohesive.** Every page looks and behaves like the same place: the same
   bar, type, colours, words, and the same answer when something is
   missing, late or broken.
3. **Quality.** Nothing half-done: every state drawn, every size and theme
   looked at, every word plain.

## Fast

A page paints its frame before any request answers, and never waits on one
request to start the next.

- **Cache first, then refresh.** Anything a page has shown before is drawn
  from the cache straight away and replaced when the fresh answer lands. A
  view that waits on the network to show what it showed a minute ago is a
  bug. Use the shared fetch in [`servers/web-shared/kit.js`](../../servers/web-shared/kit.js); don't
  write another. The one exception is the Git pages: for now they keep
  nothing and fetch nothing ahead of the click. Every view draws from what
  the forge says now, and is made fast by asking less and asking in
  parallel ([forge.md](forge.md#the-git-pages)).
- **Ask in parallel.** Requests that don't need each other's answers go out
  together. A chain of awaits is a waterfall, and the page is as slow as the
  sum of it.
- **The frame never waits.** The bar, the page's header, its tabs and its
  empty layout are in the HTML or drawn synchronously. Data fills them.
- **Get ahead of the click.** Fetch what a link will need when the pointer
  reaches it, and follow on pointer-down, not on release.
- **Small and plain.** Static files, no build step, nothing loaded from
  another site. Fonts are ours, `font-display: swap`. A page loads its own
  script and the shared ones, and nothing it does not use - a heavy piece
  (a player, a chart engine) is imported when it is first needed.
- **Measured, not felt.** Click-to-paint is measured in a headless browser
  before a page ships, cold and warm. A change that makes a page slower says
  so in its pull request and why it could not be avoided.

## Cohesive

One look, set once in [`servers/web-shared/home.css`](../../servers/web-shared/home.css) and
[`servers/web-shared/bar.css`](../../servers/web-shared/bar.css). A page reaches for a token, never a
raw value.

- **The bar** is the same on every page: the mark, the services, Docs,
  search, and who is signed in ([`servers/web-shared/shell.js`](../../servers/web-shared/shell.js)).
  A page adds to it through the search's sources, never with a bar of its
  own.
- **The footer** ends every page: the mark and the licence, Use it, Learn,
  and Policies with the security address
  ([`servers/web-shared/footer.js`](../../servers/web-shared/footer.js), its
  look in `home.css` under `.dd-foot`). `shell.js` imports it, so a page
  with the bar has it; a page without the shell loads `footer.js` itself.
  It goes after the element that holds `main`, so a page that fills the
  window (Chat) is scrolled past to reach it, and never into the app's
  own window. A page never writes a footer of its own; the gate's tests
  check every page gets this one.
- **Type**: Public Sans for words, IBM Plex Mono for anything a machine
  produced - numbers, sizes, addresses, names of things on disk. Numbers
  line up (`font-variant-numeric: tabular-nums`) and carry their unit.
- **Colour**: a toned ground, white (or dark) panels with a hairline edge,
  one green for actions and links, the bar's blue for what is selected.
  Light and dark both, from the same tokens; a page never defines a colour
  only one theme has.
- **States** have one set of words and colours everywhere, so "broken"
  looks the same on Backups as on Git:

  ```
  ok         green    working, done, healthy
  working    blue     in progress, waiting its turn
  attention  amber    needs a look: old data, nearly full, unconfirmed
  broken     red      failed, refused, unreachable
  off        grey     not running, not set up, nothing here
  ```

  A status is a word with its colour, never a colour alone:
  `<span class="state-word" data-state="attention">3 days old</span>`. The
  colours are tokens in `home.css` (`--ok`, `--working`, `--attention`,
  `--broken`, `--off`, each with a `-bg`).
- **Shape**: square-ish corners (`--radius`, 4px), hairline borders, no
  drop shadows on panels, no gradients, no hover lift, no glow. What makes a
  page look generated - soft shadows, big radii, gradient placeholders,
  coloured left borders on notes, the system font - is not used.
- **Shared pieces** live once: [`servers/web-shared/kit.js`](../../servers/web-shared/kit.js) for
  behaviour (`$`, `el`, the cache-first `load`, `prefetch`) and `home.css`
  for the look (the tokens, the state words and their `.state-word` pill).
  A piece is added there the first time a second page needs it - a number
  with its unit, an "as of" line, a loading placeholder are the next likely
  ones - and not before; a page never keeps its own copy of one.

## Quality

- **Every state is drawn.** Loading is a placeholder shaped like what is
  coming, never a spinner or "Loading…". Empty says what would be here and
  how to get some. An error says what went wrong in plain words and what to
  do. Old data says how old. A missing number is `—`, never blank or `0`.
- **Phones are the same page, narrower.** No horizontal scroll, tap targets
  big enough for a thumb, a table becomes a list of cards when it no longer
  fits.
- **Keyboard and focus.** Everything that can be clicked can be reached and
  used from the keyboard, with a visible focus ring (`--focus`).
- **Both themes, every time.** A change is looked at in light and dark, at
  phone and desktop width, before it ships - with screenshots in the pull
  request for anything visible.

## Everyone can use it

Every page meets WCAG 2.2 AA. What that asks of a page here:

- **Landmarks.** The bar is the `header`; everything else a page shows is
  in its one `main`, or a `nav` or `aside` beside it. `shell.js` puts a
  "Skip to content" link first on every page that loads it, to the `main`;
  a page without the shell carries the link itself
  (`<a class="skip-to" href="#main">`).
- **Headings.** One `h1` names the page, and levels go down one at a time:
  a panel's title under an `h1` is an `h2`, even when it is drawn small.
- **Contrast.** Words are 4.5:1 against what is behind them, in both themes.
  The quiet greys (`--ink-2`, and `--ink-3` where a page has one) are set to
  pass on `--bg`, `--sunk` and `--line-2`, not just on white.
- **Links in words are underlined.** A link inside a sentence or a
  rendered markdown page is told apart by more than its colour. Links that
  are plainly controls - tabs, lists, buttons - need not be.
- **Names.** Every field has a `label` or an `aria-label`; a button that
  is only an icon has an `aria-label`; a picture has `alt`, empty when the
  words beside it already say it. A table's column with no title has one
  for a screen reader (`<span class="sr-only">`).
- **State.** Something said after the page loads - "Saved", an error, a
  count - is in an element with `role="status"` (or `role="alert"` for an
  error). A picked link is `aria-current`; `aria-pressed` and
  `aria-selected` are for buttons and the roles that allow them.
- **Focus.** The `--focus` ring shows on everything reachable, and nothing
  sets `outline: none` without drawing its own ring. A dialog is a
  `<dialog>` opened with `showModal()` where it can be: it takes focus,
  closes on Escape and gives focus back. A menu of buttons takes focus
  when it opens, arrows move in it, Escape closes it.
- **Motion.** `home.css` turns animation and transition off for anyone
  who asks for less motion (`prefers-reduced-motion`).
- **Size.** A page reflows at 320px wide and 200% zoom with no sideways
  scroll; a part that must scroll (a log, a long description) can be
  reached from the keyboard (`tabindex="0"`). Targets are at least 24px.
- **Checked by a machine and by hand.** axe-core runs over the pages in a
  headless browser before a page ships, and the keyboard is tried by hand:
  Tab through, open and close each dialog and menu.

## Words

- Plain words, from the member's side of the screen: what they can see and
  do, not how it is built. "Your photos", not "the library gate".
- **Never a command.** No `dd`, no shell lines, on any page a member sees.
  Where something needs what a browser cannot do, say what cannot happen, or
  leave the action out.
- A button says what it does ("Remove", "Sign out on this device"), and what
  happened is said once it has ("Removed").

## How a page is built

```
servers/<service>/web/pages/<page>.html   the page: the bar's markup, its frame, no inline script
servers/<service>/web/<page>.js           its module: imports the kit, draws, fetches
servers/<service>/web/<page>.css          what the page adds to home.css, if anything
servers/web-shared/kit.js, home.css              the shared pieces: behaviour, and the look
```

- The gate serves these files as they are (`servers/gate/src/pages.rs`); the
  HTML, CSS and JS are files, never strings in Rust.
- Scripts come from our own site only and never inline: the content policy
  forbids anything else, and that is what keeps a page from running
  something it did not ship.
- The app carries copies of the pages members use (`app/carried`); a
  change to one changes its copy in the same pull request, or the flake's
  check fails.

## Adding or changing a page

1. Start from an existing page of the same kind; reuse its frame.
2. Use the kit for fetching, elements and states; add to the kit rather
   than beside it.
3. Draw from cache first; send independent requests together.
4. Draw every state: loading, empty, error, old, missing.
5. Look at it in light and dark, phone and desktop, and measure
   click-to-paint, cold and warm.
6. Put the screenshots and the measurement in the pull request.
