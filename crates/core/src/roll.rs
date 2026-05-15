//! Roll pipeline. See `CLAUDE.md` "Roll pipeline" for the canonical 6-step flow.
//! Threads an explicit [`rand::Rng`] so every drop is reproducible from a seed.

use rand::Rng;
use thiserror::Error;

use crate::affix::Affix;
use crate::item::{BaseItem, ItemInstance, Rarity};

#[derive(Debug, Error)]
pub enum RollError {
    #[error("no base items match slot {slot:?} at ilvl {ilvl}")]
    NoEligibleBase { slot: String, ilvl: u32 },
    #[error("no affix tier eligible for ilvl {ilvl}")]
    NoEligibleTier { ilvl: u32 },
}

/// Roll a single item drop. Stub: signature only — implementation comes with the
/// loot simulator in `/crates/sim`, where distributions can be eyeballed end-to-end.
pub fn roll_item<R: Rng + ?Sized>(
    _rng: &mut R,
    _bases: &[BaseItem],
    _affixes: &[Affix],
    _ilvl: u32,
    _rarity: Rarity,
) -> Result<ItemInstance, RollError> {
    todo!("implement alongside sim CLI")
}
