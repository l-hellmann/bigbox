use serde::{Deserialize, Serialize};

use crate::affix::AffixId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rarity {
    Normal,
    Magic,
    Rare,
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
