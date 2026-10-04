# What the model server and search need of a box (types:
# nix/lib/needs-type.nix). Tuning is the modules' settings, not here
# (llama-cpp.nix: dd.llm).
{
  llama-swap = {
    what = "runs one language model at a time, for the chat page and the API";
    base = {
      requires = {
        # any GPU: the prompt is read there, 2.6 times faster than on
        # node1's cores; an integrated one shares RAM, so answers are
        # written no faster (memory bandwidth, not compute)
        gpu.api = "vulkan";
        disk.kind = "ssd";
      };
      # node1, 2026-10-04: llama-swap alone, no model loaded
      uses = {
        memory = "13M";
        storage = "0";
      };
      network = {
        outbound = [ "huggingface.co" ];
        inbound = "localhost";
      };
      "with" = [ "nginx" "dd-verify" ];
      privacy = "plaintext";
    };
    variants."qwen3.6-35b" = {
      requires = {
        memory = {
          fixed = "22G";
          perKContext = null;
          locked = true;
        };
        disk.size = "22G";
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
    what = "web search for members, and the chat's search";
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
      "with" = [ "dd-verify" ];
      privacy = "plaintext";
    };
  };
}
