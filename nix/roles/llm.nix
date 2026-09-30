{ config, ... }:
{
  # The language model and its gateway. Wants the ram and the instruction
  # sets; the placement program will read those from the box's facts.
  imports = [
    ../modules/llm/llama-cpp.nix
    ../modules/llm/llm.nix
    ../modules/llm/search.nix
  ];
  dd.home.services = [
    {
      name = "Chat";
      blurb = "An AI model that runs on the box, not in someone else's cloud.";
      url = "https://llm.${config.dd.domain}/";
      # a prompt is real compute on one model: ten an hour for the demo
      # ten messages an hour for each demo, sixty for every demo together:
      # a model's answer takes the box's whole cpu for seconds
      demo = "rate:10/60";
      icon = "chat";
      color = "#2f9e6f";
      rank = 40;
    }
  ];
}
