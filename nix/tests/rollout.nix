# Staged rollout on two boxes and a relay (rollout-fleet.nix, rollout.py):
# a good release flows wave 1 -> soak -> all; one that fails its soak on
# wave 1 stops there; a failed switch after a drain keeps web up; the
# control plane going away mid-rollout holds everyone where they are.
args: import ./rollout-fleet.nix (args // { name = "rollout"; })
