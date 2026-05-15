//! Minimal combat math: damage application, dodge, armor mitigation, and
//! expected-DPS calculations. The bridge from rolled loot to "is this weapon
//! actually better?" outcomes.
//!
//! Expected-value math by default — stochastic per-shot variance belongs to
//! the realtime combat loop, not the design/balance layer. The realtime layer
//! will call the same primitives with an `Rng` threaded through.

use std::collections::HashMap;

use crate::stats::StatId;

#[derive(Debug, Clone)]
pub struct Combatant {
    pub max_life: f32,
    pub current_life: f32,
    pub armor: f32,
    pub evasion: f32,
}

impl Combatant {
    /// A naked dummy with the given hit-point pool — useful for normalizing
    /// weapon DPS to a baseline target.
    pub fn dummy(max_life: f32) -> Self {
        Self {
            max_life,
            current_life: max_life,
            armor: 0.0,
            evasion: 0.0,
        }
    }

    /// Build from a `core::aggregate_item` output map plus a character-level
    /// base life. For v1 we treat life as the only character-level base; armor
    /// and evasion start at zero and come entirely from gear.
    pub fn from_armor_stats(stats: &HashMap<StatId, f32>, base_life: f32) -> Self {
        let life = base_life + get(stats, "life");
        Self {
            max_life: life,
            current_life: life,
            armor: get(stats, "armor"),
            evasion: get(stats, "evasion"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Weapon {
    pub damage_per_shot: f32,
    pub fire_rate: f32,
    pub crit_chance: f32,
    /// Total multiplier on a crit hit. 1.5 = "crits do 150% damage."
    pub crit_multiplier: f32,
}

impl Weapon {
    /// Build a Weapon from a `core::aggregate_item` output. Sums every damage
    /// stat (intrinsic weapon_damage + bullet/incendiary/AP/explosive flats);
    /// reads fire_rate and crit stats directly.
    pub fn from_stats(stats: &HashMap<StatId, f32>) -> Self {
        let damage = get(stats, "weapon_damage")
            + get(stats, "bullet_damage")
            + get(stats, "incendiary_damage")
            + get(stats, "armor_piercing_damage")
            + get(stats, "explosive_damage");
        let crit_chance = get(stats, "crit_chance").clamp(0.0, 1.0);
        let crit_damage = get(stats, "crit_damage");
        Self {
            damage_per_shot: damage,
            fire_rate: get(stats, "fire_rate"),
            crit_chance,
            // crit_damage stat is the *bonus* above 1.0× damage. 0.5 → 1.5× crit.
            crit_multiplier: 1.0 + crit_damage,
        }
    }
}

/// Probability of a hit being dodged. Diminishing returns, capped at 50% so
/// stacking pure evasion can't make a character immortal.
pub fn dodge_chance(evasion: f32) -> f32 {
    if evasion <= 0.0 {
        return 0.0;
    }
    (evasion / (evasion + 100.0)).min(0.50)
}

/// Fraction of incoming damage absorbed by armor against a hit of given size.
/// Big hits punch through armor harder than small ones — standard PoE-flavor
/// armor model. Capped at 85% so armor isn't a hard wall.
pub fn armor_mitigation(armor: f32, hit_damage: f32) -> f32 {
    if armor <= 0.0 || hit_damage <= 0.0 {
        return 0.0;
    }
    (armor / (armor + 10.0 * hit_damage)).min(0.85)
}

/// Expected damage of a single hit from `weapon` against `target`, folding
/// crit chance × crit damage and the target's dodge + armor.
pub fn expected_hit_damage(weapon: &Weapon, target: &Combatant) -> f32 {
    let base = weapon.damage_per_shot;
    if base <= 0.0 {
        return 0.0;
    }
    let crit_factor = 1.0 + weapon.crit_chance * (weapon.crit_multiplier - 1.0);
    let post_crit = base * crit_factor;
    let dodge = dodge_chance(target.evasion);
    let mit = armor_mitigation(target.armor, post_crit);
    post_crit * (1.0 - dodge) * (1.0 - mit)
}

/// Expected sustained DPS of `weapon` against `target`.
pub fn dps_against(weapon: &Weapon, target: &Combatant) -> f32 {
    expected_hit_damage(weapon, target) * weapon.fire_rate
}

/// Expected seconds to deplete `target.current_life` with `weapon`.
/// Returns `None` when DPS is non-positive (zero damage, no fire rate, etc.).
pub fn time_to_kill(weapon: &Weapon, target: &Combatant) -> Option<f32> {
    let dps = dps_against(weapon, target);
    if dps <= 0.0 {
        None
    } else {
        Some(target.current_life / dps)
    }
}

fn get(stats: &HashMap<StatId, f32>, key: &str) -> f32 {
    stats.get(key).copied().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn naked(life: f32) -> Combatant {
        Combatant::dummy(life)
    }

    fn plain_weapon(damage: f32, rate: f32) -> Weapon {
        Weapon {
            damage_per_shot: damage,
            fire_rate: rate,
            crit_chance: 0.0,
            crit_multiplier: 1.0,
        }
    }

    #[test]
    fn naked_target_takes_full_damage() {
        let w = plain_weapon(10.0, 1.0);
        let t = naked(100.0);
        assert_eq!(expected_hit_damage(&w, &t), 10.0);
        assert_eq!(dps_against(&w, &t), 10.0);
        assert_eq!(time_to_kill(&w, &t), Some(10.0));
    }

    #[test]
    fn armor_reduces_damage_with_diminishing_returns() {
        // armor / (armor + 10 × hit_damage) — at armor 100, hit 10: 100/200 = 50%
        let mit_100 = armor_mitigation(100.0, 10.0);
        let mit_300 = armor_mitigation(300.0, 10.0);
        let mit_900 = armor_mitigation(900.0, 10.0);
        assert!(approx(mit_100, 0.50));
        assert!(approx(mit_300, 0.75));
        assert!(approx(mit_900, 0.90_f32.min(0.85))); // capped
        // 3× armor doesn't give 3× mitigation — diminishing returns hold.
        assert!(mit_300 < 3.0 * mit_100);
    }

    #[test]
    fn armor_mitigation_caps_at_85_percent() {
        assert!(approx(armor_mitigation(10_000.0, 1.0), 0.85));
        assert!(approx(armor_mitigation(1_000_000.0, 1.0), 0.85));
    }

    #[test]
    fn big_hits_punch_through_armor() {
        // Same armor: a tiny hit gets mitigated more than a huge hit.
        let small_hit = armor_mitigation(100.0, 1.0); // 100/(100+10) ≈ 0.909, capped 0.85
        let huge_hit = armor_mitigation(100.0, 100.0); // 100/1100 ≈ 0.0909
        assert!(small_hit > huge_hit);
    }

    #[test]
    fn evasion_caps_at_50_percent() {
        assert!(approx(dodge_chance(0.0), 0.0));
        assert!(approx(dodge_chance(100.0), 0.50));
        assert!(approx(dodge_chance(10_000.0), 0.50));
    }

    #[test]
    fn crit_increases_expected_damage() {
        let no_crit = plain_weapon(10.0, 1.0);
        let with_crit = Weapon {
            crit_chance: 0.5,
            crit_multiplier: 2.0,
            ..no_crit.clone()
        };
        let t = naked(100.0);
        // crit factor = 1 + 0.5 × (2.0 - 1.0) = 1.5
        assert!(approx(expected_hit_damage(&with_crit, &t), 15.0));
        assert!(expected_hit_damage(&with_crit, &t) > expected_hit_damage(&no_crit, &t));
    }

    #[test]
    fn better_weapon_kills_faster() {
        let weak = plain_weapon(10.0, 1.0);
        let strong = plain_weapon(50.0, 1.0);
        let t = naked(100.0);
        assert_eq!(time_to_kill(&weak, &t), Some(10.0));
        assert_eq!(time_to_kill(&strong, &t), Some(2.0));
    }

    #[test]
    fn weapon_from_stats_sums_damage_types() {
        let mut stats = HashMap::new();
        stats.insert("weapon_damage".into(), 12.0);
        stats.insert("bullet_damage".into(), 8.0);
        stats.insert("incendiary_damage".into(), 5.0);
        stats.insert("fire_rate".into(), 4.0);
        stats.insert("crit_chance".into(), 0.20);
        stats.insert("crit_damage".into(), 0.50);
        let w = Weapon::from_stats(&stats);
        assert!(approx(w.damage_per_shot, 25.0));
        assert!(approx(w.fire_rate, 4.0));
        assert!(approx(w.crit_chance, 0.20));
        assert!(approx(w.crit_multiplier, 1.50));
    }

    #[test]
    fn combatant_from_armor_stats() {
        let mut stats = HashMap::new();
        stats.insert("life".into(), 60.0);
        stats.insert("armor".into(), 40.0);
        stats.insert("evasion".into(), 15.0);
        let c = Combatant::from_armor_stats(&stats, 100.0);
        assert!(approx(c.max_life, 160.0));
        assert!(approx(c.current_life, 160.0));
        assert!(approx(c.armor, 40.0));
        assert!(approx(c.evasion, 15.0));
    }
}
