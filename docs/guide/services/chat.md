# Chat

## What it is

A chat assistant that runs on Commonty's own box, not at an AI company.
It can search the web for you when you ask it to. Your conversations are
kept locked in your library.

## Using it

- **In the app:** open **Chat** from the home page.
- **In a browser:** open Chat, press **Open**, and allow the browser in
  the app ([Using a browser](../browser.md)).
- **On a phone:** use the Android app. There is no app for iPhone yet.
  <!-- coming: no Mac or iPhone app yet -->

Then:

- **Ask:** type and send. The answer appears as it is written.
  **New chat** starts another.
- **Web search:** press **Search** before you send. The box searches,
  and the answer cites the pages it used as [1], [2] and so on. If the
  search finds nothing, the answer says it is from the model alone.
- **Think:** the model reasons before answering. Answers are better and
  slower.
- **Your chats** are listed beside the conversation, and you can search
  them.
- **Settings:** what it should call you, custom instructions, the
  default model, **Export** (every chat as one file), and **Delete all
  chats**. Your settings are kept locked with your chats.
- **Models:** pick one from the list. The first answer can take a moment
  while the model wakes up.

Answers can be wrong. Check anything that matters.

## Privacy

| | The box | Outside |
| --- | --- | --- |
| **Chat history** | 🔒 **Locked** | 🏠 **Stays home** |
| **Chat, while it answers** | 👁 **Open to the box** | 🏠 **Stays home** |
| **Web search** | 👁 **Open to the box** | 📖 **Read outside** |

- **Your history** is a locked file per chat in your library, like
  [Files](files.md). The box cannot read it.
- **While it answers,** the model on the box reads the conversation it is
  answering. Nothing of it is written down: the model server keeps no
  log of what you say, and has no network of its own.
- **Web search:** the box sends your question, cut to 300 characters, to
  public search engines through a search service on the box. They see
  the search, coming from the box's internet address, not yours. Only
  titles, links and a line of each result come back to the model.
- **The demo:** demo chats pass through Cloudflare, which can read them.
  They are not kept, and the demo cannot search the web.

Every service's ratings are in the [Privacy policy](../../policies/privacy.md); how Commonty is protected is in [Security](../../policies/security.md).

## Security

- **Your chats' key is your library's,** held by your devices. The Chat
  page comes from the app's signed release, so a box cannot change it to
  take the key.
- **Who can reach it:** members. The demo can try it with a limited
  number of messages. Guests cannot.
- **Answers cannot reach out.** The page shows the model's answer as
  text, with no scripts and no images, so an answer cannot make your
  browser fetch something that would tell a site what was said.
- **The search service** reaches the search engines only, never the
  house network or Commonty's network.
- **If a box were taken over,** the attacker could read conversations
  sent to it while they are answered, and the searches it sends, from
  then on. Your kept history stays locked.

## Deleting and keeping

- A deleted chat goes to your library's trash for **30 days**, then it
  is gone from every box within **90 days**, as for
  [Files](files.md#deleting-and-keeping).
- Chats go with your library when you leave ([Leaving](../leaving.md)).

## Limits

- One model runs at a time on one box, so answers are slower than a big
  AI service, and one person's long answer can make others wait.
- If this device cannot open your library yet, chats are not kept. The
  page says so.
- The model knows nothing after its training date unless you use web
  search.
- **Tools** are not built yet.
