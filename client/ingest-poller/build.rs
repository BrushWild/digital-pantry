//! Linker setup for this WSL box (non-root, no apt).
//!
//! - OpenSSL 3.5.6 prebuilt dev tree at ~/.local/openssl-dev
//!   (missing generated headers — opensslconf.h/configuration.h — were
//!   copied over from /tmp/openssl-3.5.6, the identical-version source).
//! - zlib (system /usr/lib/x86_64-linux-gnu/libz.so) and zstd
//!   (libzstd.so.1, symlinked into ~/.local/compress-dev/lib because the
//!   unversioned .so is missing).
//!
//! LDFLAGS alone does not reach the *final* link step for a binary crate,
//! so we emit rustc-link-* directives here instead.

use std::env;

fn home() -> String {
    env::var("HOME").unwrap_or_else(|_| "/opt/data/home".to_string())
}

fn main() {
    let openssl = format!("{home}/.local/openssl-dev/usr", home = home());
    let compress = format!("{home}/.local/compress-dev/lib", home = home());
    let syslib = "/usr/lib/x86_64-linux-gnu";

    println!("cargo:rustc-link-search=native={openssl}/lib/x86_64-linux-gnu");
    println!("cargo:rustc-link-search=native={compress}");
    println!("cargo:rustc-link-search=native={syslib}");
    println!("cargo:rustc-link-lib=ssl");
    println!("cargo:rustc-link-lib=crypto");
    println!("cargo:rustc-link-lib=z");
    println!("cargo:rustc-link-lib=zstd");
    println!("cargo:rustc-link-args=-Wl,-rpath,{openssl}/lib/x86_64-linux-gnu");
    println!("cargo:rustc-link-args=-Wl,-rpath,{syslib}");
    println!("cargo:rerun-if-env-changed=HOME");
}
