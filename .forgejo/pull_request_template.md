<!--
Title: <type>: <what it does>
  type is one of: feature, service, fix, security, ci, docs, refactor, deps, chore
  "WIP: " in front keeps it a draft (no full ci, no merge); a ship script
  that merges and publishes puts "release: " before the type.
The style check reads this body: each section below needs words of its own
(these comments do not count). docs: and chore: need What alone; service:
also needs Hardware and ports. Delete a section your type does not need.
-->

## What
<!-- what changes, in a few sentences -->

## Why
<!-- what was wrong or missing -->

## How it was tested
<!-- the commands run and what was looked at -->

## Risk and rollback
<!-- what could break, and how to undo it -->

## Hardware and ports
<!-- service: only. What it needs from a box (cpu, memory, disk, gpu, kvm, tpm) and the ports it listens on (docs/dev/hardware.md) -->
