# Git

## What it is

A place for code: repositories, issues, pull requests, CI runs and
releases. It is [Forgejo](https://forgejo.org) on one box, with
Commonty's own pages in front. Commonty's own code lives here too.

**Every repository is public, by design.** For code that must stay
private, use an encrypted repository (below).

## Using it

Open **Git** from the home page, in the app or in a browser. Git works
in a plain browser: it holds no keys. You need to be on Commonty's
network, or use the demo, which shows Commonty's own repository.

- **Make a repository** from the Git pages, or by pushing to a new name.
- **Push and pull** over https with Git as usual.
- **Issues and pull requests** work as on any forge. You can attach
  images, PDFs, text, logs and archives up to 100 MB.
- **CI:** a workflow that asks for the `members` runner runs your jobs
  in a sealed container with internet access.
- **Stop a run:** under Actions, a run that is waiting or running has a
  Cancel button, in the list and on its own page, if you can push to the
  repository.

### Encrypted repositories

An encrypted repository is locked on your computer before each push. The
box stores it but cannot read it. You set one up with Commonty's
command-line client, as Git's remote; how is in the
[developer pages](../../dev/forge.md#encrypted-repositories). The app
cannot make one yet.

Because the box cannot read it, an encrypted repository has no pull
requests, no code view and no CI.

## Privacy

| The box | Outside |
| --- | --- |
| 👁 **Open to the box** | 📖 **Read outside** |

- **What the box keeps, and can read:** repositories, issues, pull
  requests, comments, attachments and CI logs.
- **Who else can read it:** anyone. Every repository can be read by
  every member, and by anyone on the internet through the
  demo and the public code site, which pass through Cloudflare.
- **CI jobs** reach the internet, so whatever a job sends there goes out.
- **Encrypted repositories** are 🔒 **Locked**: the box sees only their
  size and when they change.

Every service's ratings are in the [Privacy policy](../../policies/privacy.md); how Commonty is protected is in [Security](../../policies/security.md).

## Security

- **Sign-in is Commonty's.** The gate tells the forge who you are; you
  never have a separate Git password.
- **No private repositories.** The forge refuses to make one, because
  "private" would be a promise the box cannot keep.
- **CI is kept apart.** Members' jobs run in a container under gVisor,
  with no access to the box, the house network or Commonty's network.
- **Webhooks** may go only to the internet, never to a box or the house.
- **If a box were taken over,** the attacker could read and change every
  repository on it. Encrypted ones stay locked.

## Deleting and keeping

- **Deleting** a repository removes it from the forge at once.
- **Backups:** the box's nightly backups keep it up to five weeks, and
  their snapshots a week. It is gone from every box within **90 days**.
- **Leaving:** when you leave, your own repositories and your account are
  deleted at once (30 days after, if you are removed). Work you gave to other people's repositories
  stays there, as on any open-source project. If you are the last owner
  of an organisation, hand it on, or the deletion waits. See
  [Leaving](../leaving.md).

## Limits

- Repositories count against your storage allowance.
- A repository made private before this rule stays private until its
  owner makes it public, and is still readable by the box.
- Encrypted repositories need the command-line client; the app and the
  Git pages cannot make or browse them.
