# Security and reporting

Security is what makes Commonty's privacy real. A promise that nobody can
read your data is only as good as the locks behind it, so Commonty is built
on the assumption that anything can be attacked: any box, any network, any
company it passes through, and us.

## How Commonty is built

**Zero trust.** No box in the network is trusted with your data. A box
keeps what is yours locked, and it never holds the key. Even a box that is
stolen, broken into, or run by someone with bad intentions can only hand
over locked data.

**No passwords to steal at sign-in.** You sign in with a passkey, kept on
your own device. The boxes only ever see its public half. Passwords, the
vault, is the one place with a password of its own: your master password
never leaves your device, and the box keeps only a hash of it, so make it
a long one.

**Signed, checked releases.** Every box runs only releases built from
Commonty's public code and signed. Two boxes must each build a release and
get exactly the same result, and every box checks both of their
signatures before it runs it, so one compromised machine cannot slip
something in. A box that does not come up healthy after a release goes
back to the one before by itself.

**Locked disks.** Every box's disks are encrypted. The key comes in two
halves: one sealed in the box's own security chip, the other held at
Cloudflare. A disk pulled out of a box is unreadable, and so is a box
carried away from its house.

**Open source.** All of Commonty's code is public, and so is every change
to it, at [code.commonty.org/david/commonty](https://code.commonty.org/david/commonty).
Anyone can read how it works and check that it does what this page says.

**Checked before every change.** Every change is tested before it
reaches a box: it is built and checked, scanned for libraries with known
security problems, and the boxes are driven through their failure cases
before a release goes out.

## Reporting a problem

If you find a way to break Commonty's security or privacy, please tell us
privately first, at [security@commonty.org](mailto:security@commonty.org).
Tell us what you found and how to reproduce it.

- **We answer within 3 days**, to say we have it and what happens next.
- **We fix serious problems within 30 days**, and tell you when it is
  done. If a fix needs longer, we say why and when to expect it.
- **We tell everyone.** Once a fix is out, we publish what the problem
  was, what it could have done, and what we changed.
- **We thank you**, by name if you would like, when we publish it.

## Safe harbour

If you look for problems in good faith, we will not take action against
you. That means:

- you stay away from other people's data: use your own account, or look
  around the demo. The demo runs on the real boxes, so look, but do not
  attack it;
- you do not try to take Commonty down, or make it slow for others;
- you tell us privately, and give us time to fix it before telling
  anyone else.

## What is not ours to fix

Some things Commonty uses belong to other companies, such as Cloudflare,
or the companies that carry email. Problems in their own systems are
theirs to fix, so please report those to them. If something of theirs
puts Commonty's members at risk, we still want to hear about it.
