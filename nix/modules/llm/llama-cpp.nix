{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.dd.llm;

  # nixpkgs ships a baseline x86-64 build so it runs anywhere, which means no
  # SIMD at all: system_info reported LLAMAFILE/OPENMP/REPACK and no AVX line,
  # while this CPU has avx512f/bw/vl + avx512_vnni. Prefill was running scalar.
  # Hardcoding -DGGML_AVX512=ON does not work: the derivation runs llama-server
  # at build time to generate shell completions, so it SIGILLs on any builder
  # without AVX-512 (GitHub runners are a mix). ALL_VARIANTS builds every
  # microarchitecture as a loadable backend and picks the best at runtime, so
  # the build is portable and node1 still gets the icelake path.
  llamaCppTuned = pkgs.llama-cpp.overrideAttrs (o: {
    cmakeFlags = (o.cmakeFlags or [ ]) ++ [
      "-DGGML_NATIVE=OFF"
      "-DGGML_BACKEND_DL=ON"
      "-DGGML_CPU_ALL_VARIANTS=ON"
    ];
  });
  server = lib.getExe' llamaCppTuned "llama-server";

  dir = "/tank/models";
  file = m: "${dir}/${m.localName}";
  url = m: "https://huggingface.co/${m.repo}/resolve/${m.rev}/${m.file}";
  run = "/run/dd-llm";
  port = 8081; # 8080 is taken by ente's museum

  model = lib.types.submodule (
    { name, ... }:
    {
      options = {
        name = lib.mkOption {
          type = lib.types.str;
          description = "what the chat page calls it";
        };
        description = lib.mkOption {
          type = lib.types.str;
          description = "one line on what it is good for, under its name in the picker";
        };
        repo = lib.mkOption { type = lib.types.str; };
        rev = lib.mkOption {
          type = lib.types.str;
          description = "the repository commit: `resolve/main` is mutable, and the file under it gets re-uploaded";
        };
        file = lib.mkOption { type = lib.types.str; };
        sha256 = lib.mkOption { type = lib.types.str; };
        localName = lib.mkOption {
          type = lib.types.str;
          default = "${name}-${lib.substring 0 8 cfg.models.${name}.rev}-${cfg.models.${name}.file}";
          description = "the file under ${dir}, named by revision";
        };
        context = lib.mkOption {
          type = lib.types.int;
          default = 8192;
          description = "tokens of context: it bounds the KV cache, which is the real out-of-memory vector";
        };
        args = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ ];
          description = "more llama-server flags for this model alone";
        };
        idle = lib.mkOption {
          type = lib.types.int;
          default = 3600;
          description = "seconds without a request before it is unloaded and its memory given back; 0 keeps it loaded";
        };
        aliases = lib.mkOption {
          type = lib.types.listOf lib.types.str;
          default = [ ];
          description = "other names clients may already ask for it by";
        };
      };
    }
  );
in
{
  options.dd.llm.models = lib.mkOption {
    type = lib.types.attrsOf model;
    description = "The models this box offers, by the id a request names. Each is fetched once, pinned by revision and hash, and started when first asked for; one at a time, since one fills the machine.";
  };

  config = {
    dd.llm.models."qwen3.6-35b" = {
      name = "Qwen3.6 35B";
      description = "The most capable here. About 10 words a second.";
      repo = "unsloth/Qwen3.6-35B-A3B-GGUF";
      rev = "a483e9e6cbd595906af30beda3187c2663a1118c";
      file = "Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf";
      sha256 = "707a55a8a4397ecde44de0c499d3e68c1ad1d240d1da65826b4949d1043f4450";
      # where it has been since it was first fetched, and the id clients
      # knew it by when llama-server named it after its file
      localName = "qwen3.6-35b-a3b-a483e9e6-UD-Q4_K_XL.gguf";
      aliases = [ "qwen3.6-35b-a3b-a483e9e6-UD-Q4_K_XL.gguf" ];
    };

    # reads plaintext: prompts and answers, in memory while it answers
    dd.box.plaintext = [ "llama-cpp (prompts and answers)" ];

    # Each model fetched once into the models dataset, resumed across a lossy
    # line, checked against its hash before it is named. A timer, not
    # wantedBy multi-user.target: a oneshot on the activation path would hold
    # a deploy for the whole download.
    systemd.services =
      lib.mapAttrs' (
        id: m:
        lib.nameValuePair "fetch-model-${id}" {
          description = "Fetch ${m.file} into ${dir}";
          after = [
            "network-online.target"
            "zfs-mount.service"
          ];
          wants = [ "network-online.target" ];
          path = with pkgs; [
            curl
            coreutils
          ];
          unitConfig.RequiresMountsFor = dir;
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
            TimeoutStartSec = "8h";
            Restart = "on-failure";
            RestartSec = 60;
            IOSchedulingClass = "idle";
          };
          script = ''
            set -uo pipefail
            [ -e ${file m} ] && exit 0
            mkdir -p ${dir}
            for attempt in $(seq 1 60); do
              echo "fetch attempt $attempt"
              curl -fL --retry 5 --retry-delay 10 --retry-all-errors \
                -C - -o ${file m}.part ${url m} && break
              sleep 20
            done
            echo "${m.sha256}  ${file m}.part" | sha256sum -c -
            chmod 644 ${file m}.part
            mv ${file m}.part ${file m}
          '';
        }
      ) cfg.models
      // {
        # llama-swap and the model servers it starts share a network of their
        # own: nothing else on this box - CI jobs, game guests - can reach a
        # model server's port. llama-swap checks nginx's key and drops it
        # before passing a request on, so a key on the model server would
        # refuse llama-swap itself; the namespace is what keeps others out.
        llama-swap = {
          after = [ "dd-llm-key.service" ];
          requires = [ "dd-llm-key.service" ];
          serviceConfig = {
            EnvironmentFile = "${run}/env";
            PrivateNetwork = true;
            # the weights stay resident (--mlock), which needs the limit
            # raised; there is no capability to lock memory otherwise
            LimitMEMLOCK = "infinity";
            MemoryHigh = "28G";
            MemoryMax = "32G";
            ReadOnlyPaths = [ dir ];
          };
        };
        # nginx's way in: a socket only nginx may open, carried into the
        # namespace above
        llm-proxy = {
          description = "Carry nginx's requests into the model servers' network";
          requires = [
            "llama-swap.service"
            "llm-proxy.socket"
          ];
          after = [
            "llama-swap.service"
            "llm-proxy.socket"
          ];
          unitConfig.JoinsNamespaceOf = "llama-swap.service";
          serviceConfig = {
            ExecStart = "${pkgs.systemd}/lib/systemd/systemd-socket-proxyd --exit-idle-time=10min 127.0.0.1:${toString port}";
            PrivateNetwork = true;
            DynamicUser = true;
            NoNewPrivileges = true;
            ProtectSystem = "strict";
            ProtectHome = true;
          };
        };
      };
    systemd.timers = lib.mapAttrs' (
      id: _:
      lib.nameValuePair "fetch-model-${id}" {
        wantedBy = [ "timers.target" ];
        timerConfig = {
          OnActiveSec = "5s";
          OnBootSec = "1min";
          AccuracySec = "1s";
        };
      }
    ) cfg.models;
    systemd.sockets.llm-proxy = {
      wantedBy = [ "sockets.target" ];
      listenStreams = [ "${run}/llm.sock" ];
      socketConfig = {
        SocketUser = "root";
        SocketGroup = "nginx";
        SocketMode = "0660";
      };
    };

    services.llama-swap = {
      enable = true;
      listenAddress = "127.0.0.1";
      inherit port;
      settings = {
        # nothing about what people ask is kept: no copies of requests and
        # answers for llama-swap's own pages, and the model servers' output
        # (which can include a prompt at higher verbosity) is not logged
        captureBuffer = 0;
        logLevel = "warn";
        logToStdout = "proxy";
        # the same key nginx sends; llama-swap refuses anything without it
        apiKeys = [ "\${env.LLAMA_API_KEY}" ];
        # loading 23 GB into locked memory from the disk takes a while
        healthCheckTimeout = 600;
        # the page says a model is waking itself; llama-swap would say it
        # inside the reasoning text, mixed in with the model's own
        sendLoadingState = false;
        models = lib.mapAttrs (id: m: {
          inherit (m) name description aliases;
          ttl = m.idle;
          # the model server listens on 127.0.0.1 alone; "localhost" could
          # be tried as ::1 first
          proxy = "http://127.0.0.1:\${PORT}";
          cmd = lib.concatStringsSep " " (
            [
              # llama-swap has the key in its environment to check nginx's;
              # a model server that saw it would want it too, and llama-swap
              # does not pass it on. The network namespace keeps it private.
              "${pkgs.coreutils}/bin/env -u LLAMA_API_KEY"
              server
              "--host 127.0.0.1"
              "--port \${PORT}"
              "-m ${file m}"
              "--no-mmap"
              "--mlock" # weights resident; ARC is capped to 8G to leave room
              "-t 8" # llama-bench: 5.00 vs 4.82 tok/s at 4 threads
              "-c ${toString m.context}"
              # thinking is asked for per request (chat_template_kwargs
              # enable_thinking): measured 0.8s vs 16.5s for the same answer
              "--reasoning off"
              "--no-slots" # /slots would show one person's requests to another
              "--no-webui" # the chat page is ours (box/chat)
            ]
            ++ m.args
          );
        }) cfg.models;
      };
    };
  };
}
