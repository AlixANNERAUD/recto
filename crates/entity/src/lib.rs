//! Database entities of recto, generated with `sea-orm-cli` from the migrated schema by
//! `mise run entities`. Do not edit the files in `entities/` by hand.

pub mod entities;

// Also at the crate root, as nanobom imports them (`recto_entity::users`).
pub use entities::*;
