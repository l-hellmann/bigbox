//! Affix templates as authored in RON content files. An [`Affix`] is the template;
//! a rolled instance lives on an [`crate::item::ItemInstance`] as a
//! [`crate::item::RolledAffix`].

use serde::{Deserialize, Serialize};

use crate::stats::{ModifierKind, StatId};

pub type AffixId = String;
pub type GroupId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AffixSlot {
    Prefix,
    Suffix,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Affix {
    pub id: AffixId,
    pub name_fragment: String,
    pub slot: AffixSlot,
    pub group: GroupId,
    #[serde(default)]
    pub tags: Vec<String>,
    pub allowed_categories: Vec<String>,
    pub tiers: Vec<AffixTier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffixTier {
    pub tier: u8,
    pub ilvl_required: u32,
    pub weight: u32,
    pub stats: Vec<StatRoll>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatRoll {
    pub stat: StatId,
    pub kind: ModifierKind,
    pub min: f32,
    pub max: f32,
}
