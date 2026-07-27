//! Standalone HTTP server / Docker entrypoint for d-lmdb.
//!
//! Placeholder — HTTP layer (axum/clap, serve/healthcheck subcommands) is not
//! yet implemented. This exists to prove the workspace wiring (`d-lmdb-server`
//! depending on `d-lmdb` via path) compiles end to end.

fn main() {
    println!("d-lmdb-server: HTTP layer not yet implemented");
}
