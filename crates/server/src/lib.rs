//! The recto server: configuration, sessions and the HTTP routes. The binary in `main.rs` is a
//! thin wrapper so integration tests can build the router with [`serve::router`].

pub mod config;
pub mod routes;
pub mod serve;
pub mod session;
pub mod state;
#[cfg(feature = "testing")]
mod testing;
