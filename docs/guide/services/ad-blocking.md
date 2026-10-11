# Ad blocking

## What it is

Ad and tracker blocking for every device on your home Wi-Fi. A box in
your house answers the network's name lookups and refuses the names on
the blocklists. It is [Pi-hole](https://pi-hole.net).

It is there only in a house with a box, for the members who live there.

## Using it

**Set it up once, on your router**

1. Open **Ad blocking** from the home page or the menu, and look under
   **Your home network**.
2. In your router's settings, find the **DHCP** DNS servers. Set the
   first to the address the page shows (**Copy** it), and the second to
   `9.9.9.9`.
3. Save. Devices pick it up when they next reconnect to the Wi-Fi.

The second address keeps the house online when the box is off. A device
may use it now and then, and see an ad.

**Day to day**

- **On and off:** the switch in the menu, on the Ad blocking page, or in
  Settings, **Network**. It is for the whole house.
- **Pause for** a while, and it comes back on by itself.
- **A site that blocking broke:** type its name and press **Allow**. It
  is let through for the whole house until you remove it.
- The page shows today's totals, hour by hour, and the blocklists in
  use.

Only the members who live in the house can see the page and use the
switch.

## Privacy

| The box | Outside |
| --- | --- |
| 👁 **Open to the box** | ✉️ **Envelope seen** |

- **What the box can see:** every name a device in the house looks up,
  as it answers it.
- **What the box keeps:** totals only. Nothing records which device
  looked up which site, not even a log.
- **Outside companies:** a name the box does not have an answer for is
  asked of Quad9, encrypted on the way, so your internet provider cannot
  read it. Quad9 sees each name asked, coming from the house's internet
  address. The blocklists are downloaded from GitHub.
- **The second address** on your router, `9.9.9.9`, is Quad9's. A lookup
  that goes there skips the box and is not encrypted: Quad9 and your
  internet provider can see it.

Every service's ratings are in the [Privacy policy](../../policies/privacy.md); how Commonty is protected is in [Security](../../policies/security.md).

## Security

- Pi-hole's own settings answer only on the box, behind a password only
  the gate holds. The gate lets in only the household, signed in.
- Devices on Commonty's network do not use it: they are handed Quad9,
  unfiltered. Only the box owner's own devices may use it over that
  network, if set to.
- **If the box were taken over,** the attacker could see and change the
  name lookups of the devices in the house, and send them to the wrong
  places. Pages with https would still warn about a false site.

## Deleting and keeping

Nothing about who looked up what is kept, so there is nothing to
delete. Totals and the sites you allowed stay on the box.

## Limits

- It blocks only on the house network, for devices that use the router's
  DNS. A device with its own DNS setting, or on mobile data, is not
  covered.
- Ads served from the same name as the site itself cannot be blocked
  this way.
- Turning it off applies to everyone in the house.
