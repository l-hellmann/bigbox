use serde::{Deserialize, Serialize};

use crate::affix::{Affix, AffixId};
use crate::stats::{ModifierKind, StatId};

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

    /// Step down by one rarity tier (Legendary → Epic → Rare → Common → Basic).
    /// `Basic` is the floor and returns itself.
    fn step_down(self) -> Self {
        match self {
            Rarity::Legendary => Rarity::Epic,
            Rarity::Epic => Rarity::Rare,
            Rarity::Rare => Rarity::Common,
            Rarity::Common => Rarity::Basic,
            Rarity::Basic => Rarity::Basic,
        }
    }

    /// If this rarity's tier floor can't be filled at the given `ilvl` (because
    /// no eligible affix tier is good enough), walk down to the highest rarity
    /// whose floor *is* satisfiable. Matches Diablo's level-gated drops: a
    /// requested Legendary at ilvl 30 actually drops as an Epic, not as a
    /// "Legendary" with zero affixes rolled.
    ///
    /// Returns `Basic` if no affixes are eligible at all.
    pub fn downgrade_to_satisfiable(self, ilvl: u32, affixes: &[Affix]) -> Self {
        let best_eligible_tier = affixes
            .iter()
            .flat_map(|a| a.tiers.iter())
            .filter(|t| t.ilvl_required <= ilvl)
            .map(|t| t.tier)
            .min();
        let Some(best) = best_eligible_tier else {
            return Rarity::Basic;
        };
        let mut r = self;
        while best > r.tier_floor() {
            let next = r.step_down();
            if next == r {
                return r;
            }
            r = next;
        }
        r
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseItem {
    pub id: String,
    pub name: String,
    pub category: String,
    pub slot: String,
    /// Fixed stats inherent to this base (e.g. a pistol's base damage and
    /// fire rate, an armor piece's base life/armor/evasion). Aggregated as
    /// the *base* value in the three-tier stat formula — affixes layer on top.
    #[serde(default)]
    pub intrinsic_stats: Vec<IntrinsicStat>,
    /// Attachment slot types this base supports, in display order
    /// (e.g. a pistol with `["optic", "magazine"]`). Each slot type holds
    /// at most one attachment. Empty for items that take no attachments.
    #[serde(default)]
    pub attachment_slots: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntrinsicStat {
    pub stat: StatId,
    pub value: f32,
}

/// An attachment template (static, no rolling). Slotting it onto a weapon
/// folds its `modifiers` into the weapon's stat aggregation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub rarity: Rarity,
    /// Which `BaseItem.attachment_slots` name this fits into.
    pub slot_type: String,
    /// `BaseItem.category` values this attachment is compatible with.
    pub allowed_categories: Vec<String>,
    pub modifiers: Vec<AttachmentModifier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentModifier {
    pub stat: StatId,
    pub kind: ModifierKind,
    pub value: f32,
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
    /// Player-applied upgrade level, 0..=`upgrade::MAX_UPGRADE_TIER`.
    /// Drops always start at 0; the player spends scrap to bump it.
    /// See `core::upgrade` for the scaling and cost model.
    #[serde(default)]
    pub upgrade_tier: u8,
    /// Attachment template IDs currently slotted on this item. Caller
    /// enforces slot/compatibility rules (one attachment per slot_type,
    /// matching `BaseItem.attachment_slots`). Drops have an empty list.
    #[serde(default)]
    pub attached: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RolledAffix {
    pub affix_id: AffixId,
    pub tier: u8,
    pub rolls: Vec<f32>,
}
