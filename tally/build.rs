use std::env;

fn main() {
    println!("cargo:rerun-if-changed=../assets/Tally.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("../assets/Tally.ico");
        resource
            .compile()
            .expect("failed to embed the Tally application icon");
    }
}
