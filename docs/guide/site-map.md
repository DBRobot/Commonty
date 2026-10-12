# The site, drawn

Every page and tab you can open, and every page of these docs with its
sections, drawn the way Git draws a run. Click a card to go there. Pages
marked **app** open through the Commonty app.

```sitemap
The map is drawn here for anyone signed in, member or demo.
```

## Monty, the assistant

The round button in the lower right corner of each page is Monty.
It goes by this map, and can take you to any card on it and nowhere else.

- **Say where to go:** "devices", "add a device", "wifi". When the words
  match a place, it shows it straight away.
- **Ask where something is:** "where is add a device?", "upload". It
  goes to the page and points at the button for a few seconds.
- **Ask a question:** "who can see my files?" It answers from these
  docs, says which page the answer is from, and links to it. Questions
  go to the same model as [Chat](services/chat.md), with the same limits.
  Ask in your own language and it answers in it; the docs it answers
  from are in English.
- **Show me around:** a few steps through the page you are on, with
  **Next**, **Back** and **Skip** (or the arrow keys and Escape). It
  shows itself the first time you sign in on a device.
- **Is something down?** "Is Photos up?" It says which services are up
  and since when.
- **What's new?** The latest release notes. A dot on the button means
  there are some this device has not seen.
- **A repository's pages:** "the latest actions run", "pull requests",
  "branches". On a Git page it means that repository; anywhere else, the
  fleet's own, unless you name another ("issues in tom/notes").
- **What can I do here?** It says which page you are on, what you can
  do there, and offers the tour and the docs for it.
- **Your Git:** "is the CI red?", "why did it fail?", "any PRs waiting on
  me?" It looks at the repository's latest runs, the failed job and the
  pull requests waiting on you or still open, and links to each. For
  "why", the model reads the end of the failed job's log and says what
  went wrong.
- **Your boxes and your space:** "is my box OK?" says which boxes are
  answering, up to date and backed up; "how much space do I have left?"
  says what you have used of your allowance.
- **Report a bug:** it asks what happened and what you expected, then
  drafts an issue for you to read and change. Before you open it, it
  looks the draft over for anything private (an email address, a web or
  IP address, a phone number, a password, a key, a file path, your name)
  and lists what it found; **Take them out** replaces each with
  "[removed]", or **Open it as it is**. You post it in Git yourself.
  The draft ends with the page you were on, the release and your browser,
  so the bug can be found again. Issues are public. Not in the demo.
- **From anywhere:** Ctrl+K (Cmd+K on a Mac) opens it, ready to type.
  Change the keys under **This device** in [Settings](settings.md).
- **Go there straight away:** the switch under the box. With one clear
  place, it takes you there at once; with several, it lists them.
- **The down arrow** hides the box; the button opens it again. The
  conversation stays while this tab is open, and follows you from page
  to page in it, Git and these docs too. It is kept in this browser
  alone, for this tab: never sent anywhere to be kept.
- **Tuck it away:** point just right of the button and press the arrow
  that shows; the button slides off the edge. The same spot brings it
  back.
- **Turn it off** on a device in [Settings](settings.md), under **This
  device**.
- **Now and then** it looks up from its corner and follows your pointer
  for a moment, sleepy late at night, with a hat on some holidays. Not
  if your device asks for less motion.

## What it can and cannot do

- It **cannot see the page** you are on: it knows which page it is from
  its address, never what is on it.
- It **looks up some of yours, read only, as you**: your Git (runs,
  pull requests, a failed job's log), whether your boxes are answering,
  updated and backed up, and how much of your allowance you have used.
  Nothing your devices alone can open: no file, film, photo, device or
  disk-image names, no passwords. Only a failed job's log goes to the
  model; the rest it says itself.
- It **cannot do anything for you** but go to a place on the map and
  point at a button there: it never clicks, types or sends anything (a
  bug report is posted by you, in Git), and nothing it writes can run
  on your device.
- Answers can be wrong. Check anything that matters.
