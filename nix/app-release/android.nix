# The Android download: the apk, both abis in one, unsigned (the laptop
# signs it: `dd release app`). Tauri drives the build as it does on a
# desktop, but gradle runs offline against deps.json - every maven file it
# fetched once, by hash. After changing a gradle file or a tauri version:
#   nix build --impure --expr '((builtins.getFlake "git+file://'$PWD'").legacyPackages.x86_64-linux.appRelease "0.0.0").android.mitmCache.updateScript' && ./result
{
  pkgs,
  version,
  rustToolchain,
  src,
  cargoVendor,
  androidSdk,
  netAndroid,
}:
let
  androidHome = "${androidSdk}/libexec/android-sdk";
  jdk = pkgs.jdk17;
  gradle = pkgs.gradle.override { java = jdk; };
  lock = builtins.fromTOML (builtins.readFile ../../Cargo.lock);
  versionOf = name: (pkgs.lib.findFirst (p: p.name == name) null lock.package).version;
in
pkgs.stdenv.mkDerivation (finalAttrs: {
  pname = "commonty-android";
  inherit version src;
  nativeBuildInputs = [
    rustToolchain
    pkgs.cargo-tauri
    gradle
    jdk
  ];
  mitmCache = gradle.fetchDeps {
    pkg = finalAttrs.finalPackage;
    data = ./android-deps.json;
  };
  # The lock records what a real build fetches: listing every
  # configuration instead fails on the Android plugin's variants
  preGradleUpdate = ''(eval "$buildPhase")'';
  gradleUpdateTask = "help";
  SOURCE_DATE_EPOCH = "315532800";
  ANDROID_HOME = androidHome;
  ANDROID_SDK_ROOT = androidHome;
  NDK_HOME = "${androidHome}/ndk/26.1.10909125";
  JAVA_HOME = jdk.home;
  dontUseGradleBuild = true;
  dontUseGradleCheck = true;
  dontFixup = true;
  # The crates in a place gradle can write beside: tauri's own Android
  # library and the opener plugin's are gradle projects inside them, and
  # gradle builds a project in its own directory. And the files tauri
  # writes when it builds, written ahead so gradle's dependency listing
  # sees the same projects the build does.
  postPatch = ''
    # the Android plugin keeps its own settings in the home directory
    export HOME=$(mktemp -d)
    export ANDROID_USER_HOME=$HOME/.android
    # java takes its home from the user database, which in the build
    # sandbox names a directory that does not exist
    export JAVA_TOOL_OPTIONS="-Duser.home=$HOME"
    export CARGO_VENDOR=$PWD/.vendor
    cp -rL ${cargoVendor} $CARGO_VENDOR && chmod -R u+w $CARGO_VENDOR
    mkdir -p .cargo
    sed "s|${cargoVendor}|$CARGO_VENDOR|g" ${cargoVendor}/config.toml > .cargo/config.toml
    crate() { find $CARGO_VENDOR -maxdepth 2 -type d -name "$1-$2" | head -1; }
    tauri=$(crate tauri ${versionOf "tauri"})
    opener=$(crate tauri-plugin-opener ${versionOf "tauri-plugin-opener"})
    cat > app/gen/android/tauri.settings.gradle <<EOS
    include ':tauri-android'
    project(':tauri-android').projectDir = new File("$tauri/mobile/android")
    include ':tauri-plugin-opener'
    project(':tauri-plugin-opener').projectDir = new File("$opener/android")
    EOS
    cat > app/gen/android/app/tauri.build.gradle.kts <<EOS
    val implementation = configurations.getByName("implementation")
    dependencies {
      implementation("androidx.lifecycle:lifecycle-process:2.10.0")
      implementation(project(":tauri-android"))
      implementation(project(":tauri-plugin-opener"))
    }
    EOS
  '';
  # gradle runs in the Android project, the dependency listing included
  postConfigure = "cd app/gen/android";
  buildPhase = ''
    runHook preBuild
    cd ../../..
    export CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=$NIX_BUILD_CORES
    export COMMONTY_VERSION=${version}
    # the engine's libraries, where build.rs links them and the apk carries them
    mkdir -p app/gen/android/app/src/main/jniLibs
    cp -r ${netAndroid}/. app/gen/android/app/src/main/jniLibs/
    chmod -R u+w app/gen/android/app/src/main/jniLibs
    export COMMONTY_NET_LIB_DIR=$PWD/app/gen/android/app/src/main/jniLibs
    # tauri runs gradlew; here that is the pinned gradle, offline, through
    # the cache of what it may fetch
    cat > app/gen/android/gradlew <<EOS
    #!${pkgs.runtimeShell}
    exec ${gradle}/bin/gradle $(printf '%q ' "''${gradleFlagsArray[@]}") \
      -Pandroid.aapt2FromMavenOverride=${androidHome}/build-tools/35.0.0/aapt2 "\$@"
    EOS
    chmod +x app/gen/android/gradlew
    cd app
    cargo tauri android build --ci --apk --target aarch64 --target x86_64 \
      --config '{"version":"${version}"}'
    runHook postBuild
  '';
  installPhase = ''
    install -Dm644 gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk \
      $out/commonty-unsigned.apk
  '';
})
