// SPDX-License-Identifier: Apache-2.0
//
// Keychain probe for step 5, item 7: can a signed command-line tool, as the bundled `void` is,
// read and write a generic password in the data-protection keychain under the App Group
// HAUTK68F56.dev.voidfs, with no prompt? Prints one line and exits 1 on an error.
use security_framework::passwords::{
    delete_generic_password_options, generic_password, set_generic_password_options, PasswordOptions,
};
use std::time::Instant;

const SERVICE: &str = "dev.voidfs.probe";
const ACCOUNT: &str = "probe";

fn options(group: Option<&str>) -> PasswordOptions {
    let mut o = PasswordOptions::new_generic_password(SERVICE, ACCOUNT);
    o.use_protected_keychain();
    if let Some(g) = group {
        o.set_access_group(g);
    }
    o
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let group = std::env::var("PROBE_GROUP").ok().filter(|g| !g.is_empty());
    let started = Instant::now();
    let result = match args.get(1).map(String::as_str) {
        Some("read") => generic_password(options(group.as_deref())).map(|v| format!("read {:?}", String::from_utf8_lossy(&v))),
        Some("write") => set_generic_password_options(args[2].as_bytes(), options(group.as_deref())).map(|()| "wrote".to_string()),
        Some("delete") => delete_generic_password_options(options(group.as_deref())).map(|()| "deleted".to_string()),
        _ => {
            eprintln!("usage: kcprobe read|write <value>|delete (PROBE_GROUP=<access group>)");
            std::process::exit(2);
        }
    };
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    match result {
        Ok(s) => println!("rust ok: {s} ({ms:.1} ms)"),
        Err(e) => {
            println!("rust error: OSStatus {} {e} ({ms:.1} ms)", e.code());
            std::process::exit(1);
        }
    }
}
