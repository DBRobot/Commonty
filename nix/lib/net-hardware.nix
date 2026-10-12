# What a box's network hardware means for its config, from its report
# (fleet/boxes/<box>/hardware.json: nics and stack, box/report/nics.sh).
# Builtins only, so scripts/box-report can ask it with a plain `nix eval`.
#
#   initrdModules  the wired adapters' drivers, and a usb adapter's host
#                  controller's, so the initrd has the cable (the disks'
#                  unlock over the network)
#   wifi           the Wi-Fi adapter's name, or null: no fallback without one
#   flaky          the adapters known to fail (flaky-nics.json), and why
#   warnings       what the owner should hear when the box is set up, and at
#                  every build of its config
#
# A report from before the fields were there has no bus, host or stack:
# such a box gets no host module and no stack warnings until it reports
# again.
{
  hw,
  # the house's other boxes' addresses: a fallback whose DNS asks one of
  # them first waits on that box
  houseBoxes ? { },
}:
let
  nics = hw.nics or [ ];
  wired = builtins.filter (n: !n.wireless) nics;
  wireless = builtins.filter (n: n.wireless) nics;
  known = builtins.fromJSON (builtins.readFile ./flaky-nics.json);
  # a usb adapter in a report from before hosts were recorded: on a box
  # made this decade, behind xhci
  hostOf =
    n:
    if n ? host then
      (if n.host == null then null else n.host.driver)
    else if n.usb or false then
      "xhci_hcd"
    else
      null;
  # the module that drives a host controller, by its driver's name
  hostModule = {
    xhci_hcd = "xhci_pci";
    ehci_hcd = "ehci_pci";
  };
  matches = n: k: n.driver == k.driver && (k.host or null == null || hostOf n == k.host);
  flaky = builtins.concatMap (
    n:
    map (k: {
      inherit (n) name driver;
      inherit (k) why instead;
    }) (builtins.filter (matches n) known)
  ) wired;
  unique = l: builtins.foldl' (a: x: if builtins.elem x a then a else a ++ [ x ]) [ ] l;
  stack = if hw.stack or null == null then [ ] else hw.stack;
  fallbacks = builtins.filter (l: l.role == "fallback") stack;
  boxAt = a: builtins.filter (b: houseBoxes.${b} == a) (builtins.attrNames houseBoxes);
  dnsOnBox = builtins.concatMap (
    l:
    let
      first = if l.dns == [ ] then null else builtins.head l.dns;
      at = if first == null then [ ] else boxAt first;
    in
    map (b: "${l.link} (the fallback) asks ${b} for names first (${first}): with ${b} down, every lookup there waits out a timeout before the next server") at
  ) fallbacks;
in
{
  initrdModules = unique (
    map (n: n.driver) wired
    ++ builtins.concatMap (
      n:
      let
        h = hostOf n;
      in
      if h != null && hostModule ? ${h} then [ hostModule.${h} ] else [ ]
    ) wired
  );
  wifi = if wireless == [ ] then null else (builtins.head wireless).name;
  inherit flaky;
  warnings =
    map (f: "${f.name} (${f.driver}) is ${f.why}. Better: ${f.instead}.") flaky
    ++ (
      if flaky != [ ] && wireless == [ ] then
        [ "a wired adapter known to fail, and no Wi-Fi to fall back to: when it goes silent this box is gone until someone resets it" ]
      else
        [ ]
    )
    ++ dnsOnBox;
}
