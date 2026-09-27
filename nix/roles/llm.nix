{ config, ... }:
{
  # The language model and its gateway. Wants the ram and the instruction
  # sets; the placement program will read those from the box's facts.
  imports = [
    ../modules/llm/llama-cpp.nix
    ../modules/llm/llm.nix
  ];
  dd.home.services = [
    {
      name = "Chat";
      blurb = "An AI model that runs on the box, not in someone else's cloud.";
      url = "https://llm.${config.dd.domain}/";
      # a prompt is real compute on one model: ten an hour for the demo
      demo = "rate:10";
      icon = "chat";
      color = "#2f9e6f";
      rank = 40;
    }
  ];
}
