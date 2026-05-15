//! Pure game logic. No rendering, no IO, no platform dependencies — keep it that way
//! so the loot simulator and unit tests stay headless and fast.

pub mod affix;
pub mod aggregate;
pub mod item;
pub mod roll;
pub mod stats;

pub use affix::{Affix, AffixSlot, AffixTier, StatRoll};
pub use aggregate::aggregate_item;
pub use item::{BaseItem, IntrinsicStat, ItemInstance, Rarity, RolledAffix};
pub use stats::{Modifier, ModifierKind, StatId, aggregate};
