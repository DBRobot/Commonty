# What the model server and search need of a box (types:
# nix/lib/needs-type.nix). Tuning is the modules' settings, not here
# (llama-cpp.nix: dd.llm).
{
  llama-swap = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    what = "runs one language model at a time, for the chat page and the API";
    # every member's chats, in the clear
    trust = "owner";
    traffic = {
      class = "medium";
      downMbps = 50;
    };
    units = [
      "llm-proxy"
      "fetch-model-*"
      "dd-llm-key"
      "dd-models-prune"
    ];
    base = {
      requires = {
        # any GPU: the prompt is read there, 2.6 times faster than on
        # node1's cores; an integrated one shares RAM, so answers are
        # written no faster (memory bandwidth, not compute)
        gpu.api = "vulkan";
        disk.kind = "ssd";
        # the resident 4B (Monty's, always loaded) is held whichever big
        # model is: here, since the reserve counts only the largest variant
        memory = {
          fixed = "3G";
          perKContext = null;
          locked = true;
        };
      };
      # node1, 2026-10-04: llama-swap alone, no model loaded; and the 4B's file
      uses = {
        memory = "13M";
        storage = "3G";
      };
      network = {
        outbound = [ "huggingface.co" ];
        inbound = "localhost";
      };
      depends = {
        nginx.local = true;
        dd-verify.local = true;
      };
      privacy = "plaintext";
    };
    variants."qwen3.6-35b" = {
      requires = {
        memory = {
          fixed = "22G";
          perKContext = null;
          locked = true;
        };
        disk.size.min = "22G";
      };
      uses = {
        memory = "22G";
        storage = "21G"; # du of /tank/models on node1
      };
      # a card with its own memory this big writes answers faster too
      faster.gpu = {
        api = "vulkan";
        memory = "24G";
      };
    };
  };

  searxng = {
    # switched by the control plane: it runs where the plan puts it (box/placed.nix)
    placed = true;
    what = "web search for members, and the chat's search";
    # what members search for
    trust = "owner";
    traffic = {
      class = "medium";
      downMbps = 10;
    };
    units = [
      "searx"
      "searx-init"
      "dd-search-key"
    ];
    base = {
      # node1, 2026-10-04: peak since its last start; keeps nothing on disk
      uses = {
        memory = "90M";
        storage = "0";
      };
      network = {
        outbound = "internet";
        inbound = "localhost";
      };
      depends.dd-verify.local = true;
      privacy = "plaintext";
    };
  };
}
