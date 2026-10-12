fn main() {
    // the network engine, Go (app/net): a static archive on desktop, a
    // shared library on Android (Go makes no archives there), found where
    // the build environment says. On Android the same .so goes into the
    // apk (gen/android/.../jniLibs/<abi>/), which is where the
    // COMMONTY_NET_LIB_DIR for that target points.
    println!("cargo:rerun-if-env-changed=COMMONTY_NET_LIB_DIR");
    // the release tag's version (lib.rs version())
    println!("cargo:rerun-if-env-changed=COMMONTY_VERSION");
    // The release key, and the repository whose releases branch holds the
    // signed app.json: what the installer checks the controller's card
    // against (installer.rs). From the tree in a checkout; a nix build,
    // which has only the crates, hands them over.
    for (var, file) in [
        ("COMMONTY_RELEASE_KEY", "../fleet/release.pub"),
        ("COMMONTY_REPO", "../fleet/policy.json"),
    ] {
        println!("cargo:rerun-if-env-changed={var}");
        let value = std::env::var(var).ok().or_else(|| {
            let text = std::fs::read_to_string(file).ok()?;
            println!("cargo:rerun-if-changed={file}");
            Some(if var == "COMMONTY_REPO" {
                // "repo": "owner/name", without a json parser here
                let at = text.find("\"repo\"")?;
                text[at + 6..].split('"').nth(1)?.to_string()
            } else {
                text
            })
        });
        if let Some(v) = value {
            println!("cargo:rustc-env={var}={}", v.trim());
        }
    }
    let target = std::env::var("TARGET").unwrap_or_default();
    let dir = std::env::var("COMMONTY_NET_LIB_DIR")
        .expect("COMMONTY_NET_LIB_DIR: where libcommontynet is (nix build .#net)");
    if target.contains("android") {
        let abi = if target.starts_with("aarch64") {
            "arm64-v8a"
        } else if target.starts_with("x86_64") {
            "x86_64"
        } else {
            panic!("no engine for {target}")
        };
        println!("cargo:rustc-link-search=native={dir}/{abi}");
        println!("cargo:rustc-link-lib=dylib=commontynet");
    } else if target.contains("windows") {
        // A dll here, not an archive, and the reason is the toolchain:
        // Go's cgo on Windows builds with mingw, Rust's default target
        // links with msvc, and one cannot use the other's static archive.
        // A dll has no such argument - it is loaded by name at runtime -
        // so the engine is built c-shared and ships beside the exe, the
        // same shape Android already uses and for the same reason.
        println!("cargo:rustc-link-search=native={dir}");
        println!("cargo:rustc-link-lib=dylib=commontynet");
        // what Go's runtime and net package want from Windows itself
        for l in [
            "ws2_32", "iphlpapi", "userenv", "ntdll", "bcrypt", "advapi32", "winmm",
        ] {
            println!("cargo:rustc-link-lib=dylib={l}");
        }
    } else {
        println!("cargo:rustc-link-search=native={dir}");
        println!("cargo:rustc-link-lib=static=commontynet");
        // these branch on the target, not on the machine doing the
        // building: `cfg!` here is the host, which is only the same thing
        // by luck and is wrong the moment anything cross-compiles
        if target.contains("linux") {
            println!("cargo:rustc-link-lib=dylib=pthread");
            println!("cargo:rustc-link-lib=dylib=resolv");
        }
        #[cfg(target_os = "macos")]
        {
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
            println!("cargo:rustc-link-lib=framework=Security");
            println!("cargo:rustc-link-lib=dylib=resolv");
        }
    }
    tauri_build::build()
}
