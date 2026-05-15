//! Three-tier stat aggregation: `(base + flat) × (1 + increased) × ∏(1 + more_i)`.

use serde::{Deserialize, Serialize};

pub type StatId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifierKind {
    Flat,
    Increased,
    More,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Modifier {
    pub kind: ModifierKind,
    pub value: f32,
}

pub fn aggregate(base: f32, modifiers: &[Modifier]) -> f32 {
    let mut flat = 0.0_f32;
    let mut increased = 0.0_f32;
    let mut more = 1.0_f32;
    for m in modifiers {
        match m.kind {
            ModifierKind::Flat => flat += m.value,
            ModifierKind::Increased => increased += m.value,
            ModifierKind::More => more *= 1.0 + m.value,
        }
    }
    (base + flat) * (1.0 + increased) * more
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(kind: ModifierKind, value: f32) -> Modifier {
        Modifier { kind, value }
    }

    #[test]
    fn no_modifiers_returns_base() {
        assert_eq!(aggregate(100.0, &[]), 100.0);
    }

    #[test]
    fn flat_adds_to_base() {
        let mods = [m(ModifierKind::Flat, 20.0), m(ModifierKind::Flat, 5.0)];
        assert_eq!(aggregate(100.0, &mods), 125.0);
    }

    #[test]
    fn increased_modifiers_sum_into_one_bucket() {
        let mods = [
            m(ModifierKind::Increased, 0.10),
            m(ModifierKind::Increased, 0.15),
        ];
        // (100 + 0) * (1 + 0.25) * 1.0 = 125
        assert_eq!(aggregate(100.0, &mods), 125.0);
    }

    #[test]
    fn more_modifiers_are_fully_multiplicative() {
        let mods = [m(ModifierKind::More, 0.20), m(ModifierKind::More, 0.20)];
        // 100 * 1.0 * 1.2 * 1.2 = 144
        let result = aggregate(100.0, &mods);
        assert!((result - 144.0).abs() < 1e-4, "got {result}");
    }

    #[test]
    fn full_three_tier_formula() {
        let mods = [
            m(ModifierKind::Flat, 20.0),       // base+flat = 120
            m(ModifierKind::Increased, 0.10),  // ×1.25 = 150
            m(ModifierKind::Increased, 0.15),
            m(ModifierKind::More, 0.20),       // ×1.2 = 180
        ];
        let result = aggregate(100.0, &mods);
        assert!((result - 180.0).abs() < 1e-4, "got {result}");
    }

    #[test]
    fn two_mores_beat_one_equivalent_increased() {
        // 50% increased vs two 25% more: more should win
        let inc = aggregate(100.0, &[m(ModifierKind::Increased, 0.50)]);
        let mor = aggregate(
            100.0,
            &[m(ModifierKind::More, 0.25), m(ModifierKind::More, 0.25)],
        );
        assert_eq!(inc, 150.0);
        assert!(mor > inc, "two 25% more ({mor}) should exceed 50% increased ({inc})");
    }
}
