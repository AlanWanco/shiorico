use std::{env, path::Path};

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR");
    let marker = Path::new(&manifest).join("../../.shiorico-local-eplus");
    println!("cargo:rerun-if-env-changed=SHIORICO_LOCAL_EPLUS");
    println!("cargo:rerun-if-changed={}", marker.display());
    let local = match env::var("SHIORICO_LOCAL_EPLUS").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        Ok(value) => panic!("SHIORICO_LOCAL_EPLUS must be 0 or 1, got {value:?}"),
        Err(env::VarError::NotPresent) => marker.is_file(),
        Err(error) => panic!("Invalid SHIORICO_LOCAL_EPLUS: {error}"),
    };
    println!("cargo:rustc-check-cfg=cfg(shiorico_local_eplus)");
    if local {
        println!("cargo:rustc-cfg=shiorico_local_eplus");
    }
}
