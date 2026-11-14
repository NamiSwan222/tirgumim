//! The launcher's work, with no window: packs, installs, game detection, the catalog. Tested with `cargo test`.

pub mod catalog;
pub mod detect;
pub mod installer;
pub mod manifest;
pub mod tools;
pub mod util;

#[cfg(test)]
mod tests;
