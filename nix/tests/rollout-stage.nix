# The staging run `dd release publish` asks of one box before it signs
# (docs/dev/rollout.md): the rollout fleet with node2 joined by a key in a
# simulated TPM, which takes the writer over by the planned handover while
# a member's probe of it never fails.
args:
import ./rollout-fleet.nix (
  args
  // {
    name = "rollout-stage";
    tpm = [ "node2" ];
  }
)
