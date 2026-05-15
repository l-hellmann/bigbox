//! Combat math: damage application, dodge, armor mitigation, expected-DPS,
//! and a stochastic tick-based fight loop. The bridge from rolled loot to
//! "is this weapon actually better?" outcomes.
//!
//! Two layers, sharing primitives:
//! * **Expected-value** (`expected_hit_damage`, `dps_against`, `time_to_kill`)
//!   — design/balance lens. No RNG. Deterministic.
//! * **Stochastic** (`resolve_hit`, `simulate_fight`) — gameplay lens.
//!   RNG threaded through; each shot resolves a dodge roll and a crit roll
//!   individually. This is what the realtime tick loop will call.

use std::collections::HashMap;

use rand::Rng;

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

// =====================================================================
// Stochastic combat — per-shot resolution and the tick-based fight loop.
// =====================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HitResult {
    Dodged,
    Hit { damage_dealt: f32, was_crit: bool },
}

/// One armed-fighter slot — its combatant, the weapon it's firing, and the
/// game time at which it next gets to fire. Keep this composable so the
/// realtime loop can drive the same struct.
#[derive(Debug, Clone)]
pub struct Fighter {
    pub combatant: Combatant,
    pub weapon: Weapon,
    /// Absolute time of next shot. Set to `f32::INFINITY` if `fire_rate` is 0.
    pub next_shot_at: f32,
}

impl Fighter {
    pub fn new(combatant: Combatant, weapon: Weapon) -> Self {
        let next = if weapon.fire_rate > 0.0 {
            1.0 / weapon.fire_rate
        } else {
            f32::INFINITY
        };
        Self {
            combatant,
            weapon,
            next_shot_at: next,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FightState {
    pub time: f32,
    pub player: Fighter,
    pub enemy: Fighter,
}

impl FightState {
    pub fn new(player: Fighter, enemy: Fighter) -> Self {
        Self {
            time: 0.0,
            player,
            enemy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FightOutcome {
    PlayerWins,
    PlayerLoses,
    Timeout,
}

/// Roll one shot. Dodge → crit → armor mitigation → apply damage. Target's
/// `current_life` is mutated in place; caller checks for death afterwards.
/// The `HitResult` is for UI / combat log; ignore it if you only need state.
pub fn resolve_hit<R: Rng + ?Sized>(
    rng: &mut R,
    weapon: &Weapon,
    target: &mut Combatant,
) -> HitResult {
    let dodge = dodge_chance(target.evasion);
    if dodge > 0.0 && rng.r#gen::<f32>() < dodge {
        return HitResult::Dodged;
    }
    let was_crit = weapon.crit_chance > 0.0 && rng.r#gen::<f32>() < weapon.crit_chance;
    let pre_mit = if was_crit {
        weapon.damage_per_shot * weapon.crit_multiplier
    } else {
        weapon.damage_per_shot
    };
    let mit = armor_mitigation(target.armor, pre_mit);
    let dealt = pre_mit * (1.0 - mit);
    target.current_life = (target.current_life - dealt).max(0.0);
    HitResult::Hit {
        damage_dealt: dealt,
        was_crit,
    }
}

/// Tick-based stochastic fight. Advances `state.time` by `dt` per tick; each
/// fighter fires whenever accumulated time crosses its `next_shot_at`. Returns
/// when one side dies or `max_time` elapses. Player fires first on ties.
///
/// `dt` is the simulation step (e.g. 1/60 for 60 FPS). Smaller `dt` is more
/// faithful to realtime but slower. Use the same `dt` the gameplay loop will
/// use so balance numbers line up.
pub fn simulate_fight<R: Rng + ?Sized>(
    rng: &mut R,
    state: &mut FightState,
    dt: f32,
    max_time: f32,
) -> FightOutcome {
    let player_period = shot_period(&state.player.weapon);
    let enemy_period = shot_period(&state.enemy.weapon);

    while state.time < max_time {
        state.time += dt;

        if state.time >= state.player.next_shot_at {
            resolve_hit(rng, &state.player.weapon, &mut state.enemy.combatant);
            state.player.next_shot_at += player_period;
            if state.enemy.combatant.current_life <= 0.0 {
                return FightOutcome::PlayerWins;
            }
        }

        if state.time >= state.enemy.next_shot_at {
            resolve_hit(rng, &state.enemy.weapon, &mut state.player.combatant);
            state.enemy.next_shot_at += enemy_period;
            if state.player.combatant.current_life <= 0.0 {
                return FightOutcome::PlayerLoses;
            }
        }
    }
    FightOutcome::Timeout
}

fn shot_period(weapon: &Weapon) -> f32 {
    if weapon.fire_rate > 0.0 {
        1.0 / weapon.fire_rate
    } else {
        f32::INFINITY
    }
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

    // ----- stochastic / fight loop tests -----

    use rand::{SeedableRng, rngs::StdRng};

    fn pacifist_dummy(life: f32) -> Fighter {
        Fighter::new(naked(life), plain_weapon(0.0, 0.0))
    }

    #[test]
    fn resolve_hit_deterministic_with_seed() {
        let mut a = StdRng::seed_from_u64(42);
        let mut b = StdRng::seed_from_u64(42);
        let w = Weapon {
            damage_per_shot: 20.0,
            fire_rate: 1.0,
            crit_chance: 0.5,
            crit_multiplier: 2.0,
        };
        let mut t1 = naked(1000.0);
        let mut t2 = naked(1000.0);
        for _ in 0..20 {
            let r1 = resolve_hit(&mut a, &w, &mut t1);
            let r2 = resolve_hit(&mut b, &w, &mut t2);
            assert_eq!(r1, r2);
        }
        assert_eq!(t1.current_life, t2.current_life);
    }

    #[test]
    fn dodge_and_crit_observable_over_many_rolls() {
        let mut rng = StdRng::seed_from_u64(1);
        let w = Weapon {
            damage_per_shot: 10.0,
            fire_rate: 1.0,
            crit_chance: 0.5,
            crit_multiplier: 2.0,
        };
        let mut target = Combatant {
            max_life: 1e9,
            current_life: 1e9,
            armor: 0.0,
            evasion: 100.0, // 50% dodge cap
        };
        let mut dodged = 0;
        let mut crits = 0;
        let trials = 10_000;
        for _ in 0..trials {
            match resolve_hit(&mut rng, &w, &mut target) {
                HitResult::Dodged => dodged += 1,
                HitResult::Hit { was_crit, .. } => {
                    if was_crit {
                        crits += 1;
                    }
                }
            }
        }
        let dodge_rate = dodged as f32 / trials as f32;
        // ~50% dodge; among non-dodges, ~50% crit (so ~25% of trials crit).
        let crit_rate = crits as f32 / trials as f32;
        assert!((dodge_rate - 0.50).abs() < 0.02, "dodge_rate = {dodge_rate}");
        assert!((crit_rate - 0.25).abs() < 0.02, "crit_rate = {crit_rate}");
    }

    #[test]
    fn fight_strong_player_wins() {
        let mut rng = StdRng::seed_from_u64(7);
        let mut state = FightState::new(
            Fighter::new(naked(100.0), plain_weapon(50.0, 4.0)), // 200 DPS
            Fighter::new(naked(50.0), plain_weapon(1.0, 1.0)),   // 1 DPS
        );
        let outcome = simulate_fight(&mut rng, &mut state, 1.0 / 60.0, 10.0);
        assert_eq!(outcome, FightOutcome::PlayerWins);
        assert!(state.enemy.combatant.current_life <= 0.0);
        assert!(state.player.combatant.current_life > 0.0);
    }

    #[test]
    fn fight_overmatched_player_loses() {
        let mut rng = StdRng::seed_from_u64(7);
        let mut state = FightState::new(
            Fighter::new(naked(20.0), plain_weapon(1.0, 1.0)),    // 1 DPS, 20 HP
            Fighter::new(naked(1000.0), plain_weapon(50.0, 4.0)), // 200 DPS
        );
        let outcome = simulate_fight(&mut rng, &mut state, 1.0 / 60.0, 30.0);
        assert_eq!(outcome, FightOutcome::PlayerLoses);
    }

    #[test]
    fn fight_timeout_when_neither_can_kill() {
        let mut rng = StdRng::seed_from_u64(7);
        // Player has weapon but enemy has infinite life; player can never finish.
        let mut state = FightState::new(
            Fighter::new(naked(100.0), plain_weapon(1.0, 1.0)),
            pacifist_dummy(1e9),
        );
        let outcome = simulate_fight(&mut rng, &mut state, 1.0 / 60.0, 2.0);
        assert_eq!(outcome, FightOutcome::Timeout);
        // Time advanced to (approx) max_time.
        assert!(state.time >= 2.0);
    }

    #[test]
    fn fight_outcome_deterministic_with_seed() {
        // Two independent runs with the same seed produce the same outcome
        // and the same final state.
        let setup = || {
            FightState::new(
                Fighter::new(naked(100.0), plain_weapon(8.0, 2.0)),
                Fighter::new(naked(100.0), plain_weapon(5.0, 1.5)),
            )
        };
        let mut a = StdRng::seed_from_u64(123);
        let mut b = StdRng::seed_from_u64(123);
        let mut sa = setup();
        let mut sb = setup();
        let oa = simulate_fight(&mut a, &mut sa, 1.0 / 60.0, 60.0);
        let ob = simulate_fight(&mut b, &mut sb, 1.0 / 60.0, 60.0);
        assert_eq!(oa, ob);
        assert!(approx(sa.time, sb.time));
        assert!(approx(
            sa.player.combatant.current_life,
            sb.player.combatant.current_life
        ));
        assert!(approx(
            sa.enemy.combatant.current_life,
            sb.enemy.combatant.current_life
        ));
    }

    #[test]
    fn stochastic_mean_ttk_tracks_expected() {
        // Over many seeds, average TTK should converge to life / expected_dps.
        // Use crit so individual fights vary, mean should still land close.
        let weapon = Weapon {
            damage_per_shot: 10.0,
            fire_rate: 4.0,
            crit_chance: 0.30,
            crit_multiplier: 2.0,
        };
        // expected_dps = 10 × (1 + 0.3) × 4 = 52
        // expected TTK = 200 / 52 ≈ 3.846
        let trials = 200;
        let mut total = 0.0;
        for seed in 0..trials {
            let mut rng = StdRng::seed_from_u64(seed);
            // Player has effectively infinite life and the weapon; the pacifist
            // enemy has the 200 HP we want to deplete.
            let mut state = FightState::new(
                Fighter::new(naked(1e9), weapon.clone()),
                pacifist_dummy(200.0),
            );
            let outcome = simulate_fight(&mut rng, &mut state, 1.0 / 240.0, 60.0);
            assert_eq!(outcome, FightOutcome::PlayerWins);
            total += state.time;
        }
        let mean = total / trials as f32;
        assert!((mean - 3.846).abs() < 0.30, "mean TTK = {mean}, expected ~3.85");
    }
}
