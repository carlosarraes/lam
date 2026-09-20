fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/chat/adapters/macos_process.c");
        cc::Build::new()
            .file("src/chat/adapters/macos_process.c")
            .compile("lam_chat_macos_process");
        println!("cargo:rustc-link-lib=proc");
    }
}
