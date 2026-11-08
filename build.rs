
fn main() {
    // The application manifest is in assets/phemio.manifest, built into
    // the resources below.
    // App icon (resource id 1): the Phemio drop (also drawn at runtime if the resource is missing),
    // so Explorer, Task Manager and the taskbar all show the same icon.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") && std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("phemio_res.o");
        let windres = std::env::var("WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".into());
        let ok = std::process::Command::new(&windres)
            .current_dir("assets")
            .args(["phemio.rc", "-O", "coff", "-o"])
            .arg(&out)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            println!("cargo:rustc-link-arg-bins={}", out.display());
        } else {
            println!("cargo:warning=windres failed; the exe will have no icon");
        }
    } else if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // MSVC: compile with rc.exe if available.
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("phemio.res");
        if std::process::Command::new("rc").current_dir("assets").args(["/nologo", "/fo"]).arg(&out).arg("phemio.rc").status().map(|s| s.success()).unwrap_or(false) {
            println!("cargo:rustc-link-arg-bins={}", out.display());
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/phemio.ico");
    println!("cargo:rerun-if-changed=assets/phemio.rc");
    println!("cargo:rerun-if-changed=assets/phemio.manifest");

    // The transparency helper (mglass/, a DLL that Phemio.exe carries inside
    // it) is built first, for the same target: see README → Build.
    let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let target = std::env::var("TARGET").unwrap_or_default();
    let tried = [
        root.join("mglass").join("target").join(&target).join("release").join("mglass.dll"),
        root.join("mglass").join("target").join("release").join("mglass.dll"),
    ];
    match tried.iter().find(|p| p.exists()) {
        Some(p) => {
            println!("cargo:rustc-env=PHEMIO_MGLASS_DLL={}", p.display());
            println!("cargo:rerun-if-changed={}", p.display());
        }
        None => panic!(
            "\n\nBuild the transparency helper first:\n    cd mglass && cargo build --release --target {target}\n(looked for {})\n",
            tried.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" and ")
        ),
    }
}
