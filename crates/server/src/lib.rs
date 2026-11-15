//! The recto server: configuration, sessions and the HTTP routes. The binary in `main.rs` is a
//! thin wrapper so integration tests can build the router with [`serve::router`].

// The development mocks sign every request in; a release build must never contain them.
#[cfg(all(feature = "dev-mocks", not(debug_assertions)))]
compile_error!("the `dev-mocks` feature signs everyone in and is for debug builds only");

pub mod commands;
pub mod config;
#[cfg(any(test, all(feature = "dev-mocks", debug_assertions)))]
pub mod dev;
pub mod routes;
pub mod serve;
pub mod session;
pub mod session_store;
pub mod state;
