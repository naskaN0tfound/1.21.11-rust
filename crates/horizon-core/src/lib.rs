//! Horizon's initial simulation primitives, targeting the Minecraft Java Edition version in [`version`].
//!
//! This is not a playable game. Differential checks cover only the operations
//! listed in docs/rust-parity.md; the rest of vanilla remains unimplemented.
#![forbid(unsafe_code)]

pub mod biome;
pub mod carving;
pub mod density;
pub mod geometry;
pub mod jmath;
pub mod no_ai;
pub mod noise;
pub mod paletted;
pub mod position;
pub mod random;
pub mod storage;
pub mod text;
pub mod ticks;

pub mod release;
/// The pinned Minecraft version's own facts: 26.3 by default, 1.21.11 with the `mc-1-21-11` feature.
#[cfg(not(feature = "mc-1-21-11"))]
pub mod version;
#[cfg(feature = "mc-1-21-11")]
#[path = "version_1_21_11.rs"]
pub mod version;

pub use version::VANILLA_VERSION;
