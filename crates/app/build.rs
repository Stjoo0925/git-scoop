use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo::rerun-if-changed=windows/app.rc");
    println!("cargo::rerun-if-changed=../../assets/branding/git-scoop.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }

    let manifest =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));
    let resource = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"))
        .join("git-scoop.res");
    let result = Command::new("rc.exe")
        .current_dir(manifest)
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg("windows/app.rc")
        .output()
        .expect("Windows SDK rc.exe is required; run in the MSVC developer environment");
    assert!(
        result.status.success(),
        "Windows icon compilation failed: {}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    println!("cargo::rustc-link-arg-bin=git-scoop={}", resource.display());
}
