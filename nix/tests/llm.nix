# The model servers answer nginx and nothing else: llama-swap and every
# model server it starts live in a network of their own, reached through a
# socket only nginx's group may open, and llama-swap still wants the key made
# at boot. A model of a megabyte stands in for the real one, so this is
# llama-swap waking a real llama-server and a real answer coming back.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  tiny = pkgs.fetchurl {
    url = "https://huggingface.co/ggml-org/models-moved/resolve/499bc8821c6b12b4e53c5bffcb21ec206f212d81/tinyllamas/stories260K.gguf";
    hash = "sha256-Jwy6G9UQn0LQM1D2BAYCRWBGTbFzwOOH2R8EJtO9JW0=";
  };
in
{
  name = "llm";
  node.specialArgs = { inherit self; };
  nodes.box = {
    # the gate plays no part here: without it, a change to the gate
    # is not a reason to run this again
    dd.verify.enable = false;
    imports = [
      ./box.nix
      ../modules/llm/llama-cpp.nix
      ../modules/llm/search.nix
    ];
    dd.llm.models = lib.mkForce {
      tiny = {
        name = "Tiny";
        description = "A megabyte of story model";
        repo = "ggml-org/models-moved";
        rev = "499bc8821c6b12b4e53c5bffcb21ec206f212d81";
        file = "stories260K.gguf";
        sha256 = "unused: already in place";
        localName = "stories260K.gguf";
        context = 256;
      };
    };
    # what llm.nix does on a real box: the key, and nginx's group
    users.groups.nginx = { };
    systemd.tmpfiles.rules = [
      "d /run/dd-llm 0751 root root -"
      "d /tank/models 0755 root root -"
      "L+ /tank/models/stories260K.gguf - - - - ${tiny}"
    ];
    systemd.services.dd-llm-key = {
      wantedBy = [ "multi-user.target" ];
      before = [ "llama-swap.service" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = "printf 'LLAMA_API_KEY=testkey\\n' > /run/dd-llm/env";
    };
    environment.systemPackages = [ pkgs.curl ];
    virtualisation.memorySize = 2048;
  };
}
