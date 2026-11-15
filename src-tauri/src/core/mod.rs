//! The launcher's work, with no window: packs, installs, game detection, the catalog. Tested with `cargo test`.

pub mod catalog;
pub mod check;
pub mod detect;
pub mod discover;
pub mod installer;
pub mod launch;
pub mod manifest;
pub mod swap;
pub mod tools;
pub mod util;

#[cfg(test)]
mod tests;
