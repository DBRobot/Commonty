# What the model server needs of a box (types: nix/lib/needs-type.nix).
# Tuning is the module's settings, not here (llama-cpp.nix: dd.llm).
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
        outbound = [ "huggingface.co" ];
      };
      "with" = [ "nginx" "dd-verify" ];
      privacy = "plaintext";
    };
    variants."qwen3.6-35b" = {
      requires = {
        memory = {
          fixed = "23G";
          perKContext = null;
          locked = true;
        };
        disk.size = "23G";
      };
      # a card with its own memory this big writes answers faster too
      faster.gpu = {
        api = "vulkan";
        memory = "24G";
      };
    };
  };
}
