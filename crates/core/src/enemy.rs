//! Enemy archetypes — RON-authored content describing each enemy's combat
//! profile. v1 carries just the defensive shape (life / armor / evasion)
//! plus identifying metadata. Attack stats come later when two-way fights
//! land; until then enemies are pure damage targets the sim measures TTK
//! against.

use serde::{Deserialize, Serialize};

use crate::combat::Combatant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Enemy {
    pub id: String,
    pub name: String,
    /// Loose tag for grouping (e.g. "zombie", "boss"). Not semantically
    /// load-bearing yet — useful for filtering and presentation later.
    pub category: String,
    /// Suggested encounter level. Informational for now; spawn tables will
    /// consume it when procgen lands.
    pub ilvl: u32,
    pub max_life: f32,
    pub armor: f32,
    pub evasion: f32,
    /// XP awarded to the player on kill. See `core::progression` for the
    /// curve. Defaults to 0 so missing values fail safe — content errors
    /// surface as "no XP gained" instead of accidental free progress.
    #[serde(default)]
    pub xp_value: u32,
}

impl Enemy {
    pub fn as_combatant(&self) -> Combatant {
        Combatant {
            max_life: self.max_life,
            current_life: self.max_life,
            armor: self.armor,
            evasion: self.evasion,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_combatant_seeds_current_life_at_max() {
        let e = Enemy {
            id: "test".into(),
            name: "Test".into(),
            category: "zombie".into(),
            ilvl: 1,
            max_life: 120.0,
            armor: 15.0,
            evasion: 5.0,
            xp_value: 25,
        };
        let c = e.as_combatant();
        assert_eq!(c.max_life, 120.0);
        assert_eq!(c.current_life, 120.0);
        assert_eq!(c.armor, 15.0);
        assert_eq!(c.evasion, 5.0);
    }
}
