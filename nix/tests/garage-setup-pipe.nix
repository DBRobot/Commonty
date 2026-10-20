# garage-setup.sh reads garage's whole answer before it looks inside it.
# `garage layout history | grep -q` closes the pipe at the first match, and
# garage panics on the next write and aborts: a SIGABRT and a core dump on
# about one run in six, on every box. The garage here answers like the real
# one, match first, then more lines a moment later, and notes any write
# that finds its stdout closed.
{ pkgs, ... }:
pkgs.runCommand "garage-setup-pipe"
  {
    nativeBuildInputs = [
      pkgs.bash
      pkgs.coreutils
      pkgs.gnugrep
    ];
  }
  ''
    mkdir bin
    cat > bin/garage <<'SH'
    #!${pkgs.bash}/bin/bash
    trap "" PIPE
    answer() {
      printf '%s\n' "$1"
      sleep 0.3
      for i in $(seq 1 50); do
        printf 'line %s\n' "$i" || { echo "garage $ARGS: stdout closed" >> "$BROKEN"; exit 134; }
      done
    }
    ARGS="$*"
    case "$ARGS" in
      status) ;;
      "node id -q") echo 0123456789abcdef0123@127.0.0.1:3901 ;;
      "layout show") answer $'0123456789abcdef  node1  dc1  1.0 TB\nCurrent cluster layout version: 1' ;;
      "layout history") answer "Your cluster is currently in a stable state with a single live layout version." ;;
      *) ;;
    esac
    SH
    chmod +x bin/garage
    export PATH=$PWD/bin:$PATH BROKEN=$PWD/broken ZONE=dc1 CAPACITY=1T
    touch broken
    bash ${../modules/storage/garage-setup.sh}
    sleep 1
    if [ -s broken ]; then
      echo "garage-setup.sh closed garage's stdout before garage was done:" >&2
      cat broken >&2
      exit 1
    fi
    touch $out
  ''
