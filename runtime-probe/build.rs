fn main() { println!("cargo:rustc-env=RP_TARGET={}", std::env::var("TARGET").unwrap()); }
