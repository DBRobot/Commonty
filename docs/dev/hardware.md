# What each service needs

Status: audit, 2026-10-04, for review. Every hardware and placement
requirement found in the repo, so the discovery script reports all of them
and placement can match roles to boxes. Evidence is file:line as of that
date.

Today's boxes, for scale:

```
node1  Intel i7-1165G7, 8 threads, 62G, 1T NVMe (tank) + 4T WD Red HDD in a USB dock (vault),
       Iris Xe (VAAPI), wired USB NIC + wifi, public (tunnel), every role
node2  AMD Ryzen 5 7520U, 8 threads, 14G, 512G NVMe (tank), Radeon 610M (unused),
       wired USB NIC + wifi, core storage runner net
```

## 1. What discovery has to find

Detected today (nix/modules/box/report/detect.sh): host id, boot disk by
id, every NIC (report/nics.sh: driver, bus, ids, host controller, speed,
carrier, Wi-Fi bands) and how the box uses each (primary, fallback, DNS),
one wifi interface, first GPU render
node and driver, the TPM secrets key. facts.sh (metrics) separately reads CPU
model and flags, cores, RAM, disk kinds, GPUs, but unsigned.

Everything a role below depends on:

```
firmware   UEFI; Secure Boot state (user mode, own keys enrolled); TPM 2.0 present,
           ECC P-256, sha256 PCR bank; LoaderDevicePartUUID set (systemd-boot/lanzaboote)
cpu        arch (x86_64 only today); model; threads; flags avx2 avx512f avx512_vnni
           amx_tile sha_ni aes; vendor (kvm-intel / kvm-amd)
virt       /dev/kvm usable; nested KVM (games test); user namespaces;
           cgroup v2 memory + devices controllers; gVisor runsc works
memory     total, available; the ARC cap in effect
gpu        every render node: driver (i915/xe/amdgpu/...), VAAPI encode/decode profiles
           (vainfo), Vulkan device, shared or own VRAM
disks      every disk: by-id, nvme/sata ssd/hdd/usb, size, SMART health and wear;
           every pool: name, vdevs, redundancy, encrypted, free; which datasets exist
network    wired NICs (MAC, driver, usb, link speed) and whether the driver is in the
           initrd; wifi; house LAN address + router MAC + gateway; tailnet up;
           upstream speed up/down; latency to the relay; reachable from outside
           (public v4 / CGNAT / v6 / forwardable); outbound to the hosts roles need
power      battery present (its UPS), on AC, lid; sleep allowed or not
clock      NTP synced and offset (validators reject >2 s; unlock signs a timestamp)
firewall   iptables backend (several roles add iptables rules) vs nftables
```

## Booting on a stock PC (decided 2026-10-04)

The TPM, sealed to our signed boot, is what protects the disks. Getting our
boot loader to run on a PC with Secure Boot on (most ship that way):

- now: shim + MOK. The box boots a distro's Microsoft-signed shim, and the
  owner confirms our key once on a blue screen at first boot. No BIOS, no cost.
- later: NixOS's own Microsoft-signed shim (nixos-shim, through rhboot/shim-review)
  once it exists. That means no prompt at all.
- not doing: our own shim through review. It needs an EV certificate, a few
  hundred dollars a year.
- node1 and node2 keep their own enrolled keys (lanzaboote).

Other BIOS settings a box may need, all checked by discovery: virtualization
(games, CI), TPM on (PPI can ask with one key at boot), power on after an
outage (can't be read from Linux), boot from the internal disk. On business
PCs, Linux can change these (think-lmi, dell-wmi-sysman, hp-bioscfg).
node1's HP and node2's Acer can't.

## 2. Every box needs (core + boot)

- UEFI, an ESP of 1G (lanzaboote keeps 8 generations), Secure Boot with our
  keys enrolled by hand at the firmware screen (secure-boot.nix:3-17).
- TPM 2.0 at /dev/tpmrm0 with ECC P-256, PCR 7 sha256: the disk unlock,
  the control plane's signatures, the box report, the secrets key
  (disk-unlock-boot.sh:62-109, control main.go:117-144, report.nix).
- A **wired NIC whose driver is in the initrd**: only r8152, cdc_ether,
  cdc_ncm, ax88179 (USB adapters) are there (disk-unlock.nix:124-133), and
  the driver of each wired NIC the box reported, with a USB adapter's host
  controller (nix/lib/net-hardware.nix): an onboard PCIe NIC is in its
  box's initrd once the box has reported it. Wifi can't unlock (its
  password is on the locked disk).
- The house's static address, router and router MAC (home proven by the
  router's MAC, disk-unlock-boot.sh:51-57); internet to the unlock Worker; a
  roughly right RTC (no NTP in the initrd).
- x86_64 (flake.nix:38-40). One SSD boot disk (ashift 12, autotrim).
- Accurate time for voting boxes (2 s, validator.go:32).
- Tailnet: the agent fetches releases from git.<domain>, which only
  resolves on the owner's tailnet (core.nix:40).

## 3. Per role

```
role      hardware                                   storage                         network                          must sit with
--------  -----------------------------------------  ------------------------------  -------------------------------  --------------------------
gateway   -                                          -                               the public front door: cloudflared  headscale, the gate, nginx,
                                                                                     tunnel (CGNAT-safe), 80/443,       garage (s3. proxies 127.0.0.1:3900)
                                                                                     dnsmasq :53 on tailscale0; ONE
                                                                                     per fleet (*.domain points at it)
storage   ZFS                                        a dataset for garage blocks;    3900 all ifs, 3901 tailnet peers   -
          (garage)                                   meta on SSD (LMDB); capacity
                                                     string per box; rf=2 means the
                                                     smallest box caps usable space
photos    -  (build: 5 Next.js apps + wasm,           postgres on tank; blobs in      api/s3 through the gateway;        postgres on its own box, its
          heavy for attesters)                       garage                          -                                  dumps there; the gate
vault     -                                          postgres; the pool at dd.box.s3 tailnet only (Send public);        the pool (dd.box.s3),
                                                     (DATA_FOLDER);                  mail. out                          the gate, postgres
                                                     pwned list ~90G, peak ~180G,
                                                     1M files, under /vault
media     -                                          - (disk images are in the       tailnet; unlimited upload          the gate
                                                     libraries bucket)
library   -                                          garage bucket via s3.<domain>   -                                  the gate (runs inside it)
transcode GPU with VAAPI H.264 encode (Intel only    none: RAM (tmpfs), 256M per     127.0.0.1:4190/4191; out to         the gate, nginx
          today; AMD ignored), else all cores;       session, 4 sessions; MemoryMax  s3.<domain>
          cgroup v2 devices                          4G
llm       RAM >= ~32G free (23G model mlocked,       /tank/models ~23G on SSD;       out to huggingface; unix socket    -
          MemoryMax 32G); AVX2 at least (AVX-512     loads within 600 s
          VNNI measured faster); GPU: Vulkan,
          Intel only gets the split flags
search    -                                          -                               out to search engines; 127.0.0.1:8888  -
game      x86_64 + /dev/kvm (TCG only for tests);    /var/lib/dd-games: installs     ports 27000-27199 + fixed ones;    -
          6-8G RAM and 4 cores per world, 20G        land on the host dataset (GBs   commonty0 only; out to Steam CDN,
          budget (excludes qemu overhead);           each), 64G sparse qcow2 each    GitHub
          32-bit libs in the guest (steamcmd, wine)
forge     -                                          /vault/forgejo (node1: HDD over ssh :22 + git.<domain>;         postgres (shared), the gate,
                                                     USB), postgres                  github mirror out                  nginx/ACME
runner    /dev/kvm (VM tests); nested KVM (games     /nix/store grows (every box's   out to github (checkout),          garage on the same box (cache
          test); 2 jobs; >=4 CPUs, 8G for            closure built on every box);    git.<domain>                       127.0.0.1:3900 + storage's keys)
          members' 2 jobs; docker + gVisor;          1.5G VM images; /var/lib/docker
          iptables backend
observe   SMART-readable disks (each box's           /vault/backups (dumps),         mail. out                          storage (thanos objstore is local
          reported disks)                            /var/lib/thanos-*                                                  garage), the gate, gateway (ACME)
adblock   -                                          -                               :53 tcp/udp on every interface;    the gate; the box must be in the
                                                                                     the house LAN address; DoT out     house whose router hands it out
net       /dev/net/tun                               -                               udp 41642 all ifs; headscale out   -
```

## 4. Co-location, and where it is said

Nothing in the code assumes a particular box any more. What has to share a
box is said in the needs (`depends.<svc>.local`), and what does not is
reached over the fleet's network:

1. **Databases stay with their service.** Each box dumps the databases its
   own postgres holds, onto its own pool (storage/postgres-backup.nix). The
   storage ledger reads no other service's database: the forge and the vault
   count their members' bytes into their own boxes' heartbeats
   (storage/usage.nix), and every box adds those up (storage/view.nix).
2. **The storage pool by address, not localhost.** `dd.box.s3` is the pool
   at this box's address for it: its own garage, or a storage box's over the
   fleet's network - one in the same house when there is one, then any
   (flake.nix, `poolFor`). The vault's files, the nix cache, backups and the
   gateway's `s3.` all use it. Only what lives with garage (its own setup,
   the storage role's Thanos store) names 127.0.0.1.
3. **The gate is on every box** (`everyBox`), and every service that asks it
   about a request says `depends.dd-verify.local`.
4. **Alerts need no secret** (mail.nix): a box signs each mail with its TPM
   and the mail Worker sends it, so observe needs neither Photos nor the
   vault beside it.
5. **Datasets in the box's own pool.** Each service's dataset is made in
   `dd.box.bulkPool`, from the box's report; `/vault/...` is only the name
   of the mountpoint, and works on any pool.
6. **One gateway, by role.** The front door is the `gateway` role in
   `fleet/boxes.json`; control.<domain>, the fleet's names (fleet-names.nix) and the
   directory follow that role, never a box's name.

## 5. Things the audit found wrong (separate fixes)

- **Games ports.** Many games use fixed ports outside 27000-27199. QEMU
  forwards them but the firewall never opens them: Satisfactory 7777 + 8888,
  Rust 28016/28082, Palworld 25575, and about 25 more. Satisfactory's 8888
  also clashes with SearXNG. Games declare no `dd.ports`, so the hourly port
  check will flag every running world. The pool can also hand out a fixed
  port twice.
- **Game runtimes missing from the guest.** dotnet (cryofall, resonite,
  v-rising-bepinex), mono (neosvr, subnautica) and java (rising-world legacy).
- **Game memory budget.** It ignores qemu's overhead. With the LLM's 32G and
  the 8G ARC, node1 can overcommit.
- **transcode** keeps a whole film's HLS segments in tmpfs and never deletes
  them, so one long, high-bitrate film can hit MemoryMax 4G or fill /run.
- **AMD GPUs are ignored**: node2's Radeon goes unused for both VAAPI and
  Vulkan.
- **Ambiguity with several NICs.** detect.sh keeps the last wired NIC it
  sees.
- **The "port forward" comment in home-network.nix.** It contradicts the
  CGNAT note in public.nix.
