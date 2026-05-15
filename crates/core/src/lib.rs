//! Pure game logic. No rendering, no IO, no platform dependencies — keep it that way
//! so the loot simulator and unit tests stay headless and fast.

pub mod affix;
pub mod aggregate;
pub mod combat;
pub mod item;
pub mod roll;
pub mod stats;
pub mod upgrade;

pub use affix::{Affix, AffixSlot, AffixTier, StatRoll};
pub use aggregate::aggregate_item;
pub use combat::{
    Combatant, FightOutcome, FightState, Fighter, HitResult, Weapon, dps_against,
    expected_hit_damage, resolve_hit, simulate_fight, time_to_kill,
};
pub use item::{BaseItem, IntrinsicStat, ItemInstance, Rarity, RolledAffix};
pub use stats::{Modifier, ModifierKind, StatId, aggregate};
