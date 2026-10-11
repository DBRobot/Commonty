# Movies & TV

## What it is

Your films and shows, on a shelf with posters, and where you stopped in
each. They live in the same locked library as [Files](files.md).

## Using it

- **In the app:** open **Movies & TV** from the home page.
- **In a browser:** open Movies & TV, press **Open**, and allow the
  browser in the app ([Using a browser](../browser.md)).
- **On a phone:** use the Android app. There is no app for iPhone yet.
  <!-- coming: no Mac or iPhone app yet -->

Then:

- **Add a film:** **Add films**, and pick the files.
- **Add a show:** **New show**, then **Add episodes**. Seasons can be
  folders.
- **Watch:** pick a title and press play. **Continue watching** picks up
  where you stopped, on any of your devices. **Start over** goes back to
  the beginning.
- **Remove:** **Remove from library** (or **Remove show**). It goes to
  the trash in Files, where you can restore it.

## Privacy

| The box | Outside |
| --- | --- |
| 🔒 **Locked** | ✉️ **Envelope seen** |

- **What the box keeps:** your films, the shelf's details and where you
  stopped, all locked. It sees sizes, dates and when you watch.
- **While a film plays:** the box unlocks that one film in its memory,
  converts it for your browser, and forgets it when you stop (👁 **Open
  to the box** for that film, while it plays). It gets keys that open
  that one film and nothing else in your library. Nothing is written to
  disk. This happens for every film you play, on a computer or a phone,
  whenever the box can.
- **If the box cannot play it,** a film up to 256 MiB opens whole on your
  device instead. Larger ones can be saved and played on your device.
- **TMDB:** your device asks TMDB for a poster with only the title and
  year. TMDB sees that title and your device's internet address, not
  your library. A title TMDB does not know gets a still from the film
  itself. Posters are refreshed after five months and dropped after six,
  as TMDB's terms ask.

Every service's ratings are in the [Privacy policy](../../policies/privacy.md); how Commonty is protected is in [Security](../../policies/security.md).

## Security

- Keys, the page's code and who can open the library are as for
  [Files](files.md#security).
- **While a film plays,** the box holds that film's keys in memory. A box
  that was taken over could read the films you play on it while you
  play them, and nothing else of your library.
- A film's stream is reached by a long random address made for that
  viewing. It ends after 10 minutes without use, or when you stop.

## Deleting and keeping

As for [Files](files.md#deleting-and-keeping): the trash keeps a removed
film **30 days**, and everything is gone from every box within **90
days**. Where you stopped in each film is replaced at each save, not
kept in the trash.

The Leave tab in Settings downloads Files and Movies & TV as one zip.

## Limits

- A box plays at most four films at once. Starting a film ends any other
  film you were playing.
- Only the first picture and sound tracks play; subtitles are not shown
  yet.
- Films with keyframes far apart are converted on the box, which can be
  slow to start on a box without a graphics chip.
- Sharing a film or a shelf with another member is not built yet.
