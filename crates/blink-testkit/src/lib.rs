//! Test helpers shared by Blink's crates: the corpus manifest (SPEC §11.3) and synthetic
//! fixtures (§11.3: orientations, pairs, colour-space-tagged previews, sidecars…).
//! Dev-only: never a dependency of a shipped binary.

pub mod corpus;
pub mod fixtures;
pub mod sidecars;
