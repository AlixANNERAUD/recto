//! The recto server: configuration and the HTTP routes. The binary in `main.rs` is a thin
//! wrapper so integration tests can build the router with [`serve::router`].

pub mod config;
pub mod serve;
