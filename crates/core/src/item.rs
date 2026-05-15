use serde::{Deserialize, Serialize};

use crate::affix::AffixId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rarity {
    Basic,
    Common,
    Rare,
    Epic,
    Legendary,
}

impl Rarity {
    pub const ALL: [Rarity; 5] = [
        Rarity::Basic,
        Rarity::Common,
        Rarity::Rare,
        Rarity::Epic,
        Rarity::Legendary,
    ];

    /// Stable index for array-keyed accumulators.
    pub fn index(self) -> usize {
        match self {
            Rarity::Basic => 0,
            Rarity::Common => 1,
            Rarity::Rare => 2,
            Rarity::Epic => 3,
            Rarity::Legendary => 4,
        }
    }

    /// Best minimum tier this rarity is allowed to roll. Tiers are numbered
    /// T1 (best) → T4 (worst); an Epic with floor 3 cannot roll T4 affixes.
    /// `u8::MAX` means "no floor — any eligible tier is allowed".
    pub fn tier_floor(self) -> u8 {
        match self {
            Rarity::Basic | Rarity::Common | Rarity::Rare => u8::MAX,
            Rarity::Epic => 3,
            Rarity::Legendary => 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseItem {
    pub id: String,
    pub name: String,
    pub category: String,
    pub slot: String,
    pub affix_pool: String,
}

/// What actually dropped. Always carries its `seed` — enables compact saves,
/// debugging, shareable item codes, and deterministic replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemInstance {
    pub base: String,
    pub ilvl: u32,
    pub rarity: Rarity,
    pub seed: u64,
    pub prefixes: Vec<RolledAffix>,
    pub suffixes: Vec<RolledAffix>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolledAffix {
    pub affix_id: AffixId,
    pub tier: u8,
    pub rolls: Vec<f32>,
}
