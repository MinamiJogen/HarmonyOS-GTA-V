use std::{env, fs, path::PathBuf};
fn main() -> wasmtime::Result<()> {
    let args: Vec<_> = env::args().collect();
    let target = args.get(1).map(String::as_str).unwrap_or("aarch64-unknown-linux-ohos");
    let out = PathBuf::from(args.get(2).map(String::as_str).unwrap_or("artifacts"));
    fs::create_dir_all(&out)?;
    let mut config = wasmtime::Config::new();
    config.target(target)?.wasm_memory64(true).wasm_threads(true).shared_memory(true);
    let engine = wasmtime::Engine::new(&config)?;
    for (name, wat) in [
        ("add", r#"(module (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))"#),
        ("memory64", r#"(module (memory (export "memory") i64 1 2)
            (func (export "add") (param i32 i32) (result i32)
                i64.const 0 local.get 0 local.get 1 i32.add i32.store
                i64.const 0 i32.load))"#),
        ("shared", r#"(module (memory (export "memory") 1 2 shared)
            (func (export "add") (param i32 i32) (result i32)
                i32.const 0 local.get 0 i32.atomic.store
                i32.const 0 local.get 1 i32.atomic.rmw.add drop
                i32.const 0 i32.atomic.load))"#),
        ("shared-memory64", r#"(module (memory (export "memory") i64 1 2 shared)
            (func (export "add") (param i32 i32) (result i32)
                i64.const 0 local.get 0 i32.atomic.store
                i64.const 0 local.get 1 i32.atomic.rmw.add drop
                i64.const 0 i32.atomic.load))"#),
    ] {
        let artifact = engine.precompile_module(wat.as_bytes())?;
        fs::write(out.join(format!("{name}.cwasm")), &artifact)?;
        fs::write(out.join(format!("{name}.wat")), wat)?;
        println!("{} {} {} bytes", target, name, artifact.len());
    }
    Ok(())
}
