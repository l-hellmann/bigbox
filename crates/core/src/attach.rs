//! Attachment slot validation. `try_attach` is the single entry point that
//! the UI / game loop calls when the player drops an attachment onto a
//! weapon — enforces the three slot rules atomically (no partial state on
//! failure).
//!
//! Rules:
//! 1. The base item declares the attachment's `slot_type` in its
//!    `attachment_slots` list.
//! 2. The attachment's `allowed_categories` includes the base's `category`
//!    (e.g. an Extended Mag refuses to fit a heavy weapon).
//! 3. The base doesn't already have an attachment occupying that slot type
//!    (one attachment per slot type).
//!
//! Detach is intentionally not in v1 — when it lands, it'll just remove a
//! single attached id; no validation needed.

use thiserror::Error;

use crate::item::{Attachment, BaseItem, ItemInstance};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AttachError {
    #[error("attachment `{0}` not found in the provided attachments slice")]
    UnknownAttachment(String),

    #[error("base has no `{slot_type}` slot (available: {available:?})")]
    NoSuchSlot {
        slot_type: String,
        available: Vec<String>,
    },

    #[error("attachment `{attachment_id}` not compatible with category `{category}` (allows: {allowed:?})")]
    CategoryNotAllowed {
        attachment_id: String,
        category: String,
        allowed: Vec<String>,
    },

    #[error("slot `{slot_type}` is already occupied by `{current}`")]
    SlotAlreadyOccupied {
        slot_type: String,
        current: String,
    },
}

/// Slot the attachment onto `item` if all rules pass. Mutates `item.attached`
/// only on success — on `Err`, `item` is left untouched.
pub fn try_attach(
    item: &mut ItemInstance,
    base: &BaseItem,
    attachment_id: &str,
    attachments: &[Attachment],
) -> Result<(), AttachError> {
    let new = attachments
        .iter()
        .find(|a| a.id == attachment_id)
        .ok_or_else(|| AttachError::UnknownAttachment(attachment_id.to_string()))?;

    if !base.attachment_slots.iter().any(|s| s == &new.slot_type) {
        return Err(AttachError::NoSuchSlot {
            slot_type: new.slot_type.clone(),
            available: base.attachment_slots.clone(),
        });
    }

    if !new.allowed_categories.iter().any(|c| c == &base.category) {
        return Err(AttachError::CategoryNotAllowed {
            attachment_id: new.id.clone(),
            category: base.category.clone(),
            allowed: new.allowed_categories.clone(),
        });
    }

    for attached_id in &item.attached {
        let Some(existing) = attachments.iter().find(|a| &a.id == attached_id) else {
            continue;
        };
        if existing.slot_type == new.slot_type {
            return Err(AttachError::SlotAlreadyOccupied {
                slot_type: new.slot_type.clone(),
                current: existing.id.clone(),
            });
        }
    }

    item.attached.push(new.id.clone());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{AttachmentModifier, Rarity};
    use crate::stats::ModifierKind;

    fn pistol() -> BaseItem {
        BaseItem {
            id: "pistol".into(),
            name: "Pistol".into(),
            category: "rapid_fire".into(),
            slot: "weapon".into(),
            intrinsic_stats: vec![],
            attachment_slots: vec!["optic".into(), "magazine".into()],
        }
    }

    fn shotgun() -> BaseItem {
        BaseItem {
            id: "shotgun".into(),
            name: "Shotgun".into(),
            category: "heavy".into(),
            slot: "weapon".into(),
            intrinsic_stats: vec![],
            attachment_slots: vec!["optic".into(), "barrel".into()],
        }
    }

    fn empty_item(base_id: &str) -> ItemInstance {
        ItemInstance {
            base: base_id.into(),
            ilvl: 60,
            rarity: Rarity::Basic,
            seed: 0,
            prefixes: vec![],
            suffixes: vec![],
            upgrade_tier: 0,
            attached: vec![],
        }
    }

    fn att(id: &str, slot: &str, allowed: &[&str]) -> Attachment {
        Attachment {
            id: id.into(),
            name: id.into(),
            rarity: Rarity::Common,
            slot_type: slot.into(),
            allowed_categories: allowed.iter().map(|s| (*s).into()).collect(),
            modifiers: vec![AttachmentModifier {
                stat: "crit_chance".into(),
                kind: ModifierKind::Flat,
                value: 0.05,
            }],
        }
    }

    #[test]
    fn happy_path_attaches() {
        let mut item = empty_item("pistol");
        let attachments = vec![att("red_dot", "optic", &["rapid_fire", "heavy"])];
        let result = try_attach(&mut item, &pistol(), "red_dot", &attachments);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(item.attached, vec!["red_dot".to_string()]);
    }

    #[test]
    fn unknown_id_errors_without_mutating() {
        let mut item = empty_item("pistol");
        let result = try_attach(&mut item, &pistol(), "ghost", &[]);
        assert_eq!(
            result,
            Err(AttachError::UnknownAttachment("ghost".into()))
        );
        assert!(item.attached.is_empty());
    }

    #[test]
    fn base_missing_slot_errors() {
        // Pistol has no barrel slot.
        let mut item = empty_item("pistol");
        let attachments = vec![att("long_barrel", "barrel", &["rapid_fire", "heavy"])];
        let result = try_attach(&mut item, &pistol(), "long_barrel", &attachments);
        assert_eq!(
            result,
            Err(AttachError::NoSuchSlot {
                slot_type: "barrel".into(),
                available: vec!["optic".into(), "magazine".into()],
            })
        );
        assert!(item.attached.is_empty());
    }

    #[test]
    fn category_mismatch_errors() {
        // Shotgun has the optic slot but this attachment only allows rapid_fire.
        let mut item = empty_item("shotgun");
        let attachments = vec![att("rapid_only_sight", "optic", &["rapid_fire"])];
        let result = try_attach(&mut item, &shotgun(), "rapid_only_sight", &attachments);
        assert_eq!(
            result,
            Err(AttachError::CategoryNotAllowed {
                attachment_id: "rapid_only_sight".into(),
                category: "heavy".into(),
                allowed: vec!["rapid_fire".into()],
            })
        );
        assert!(item.attached.is_empty());
    }

    #[test]
    fn slot_collision_errors() {
        let mut item = empty_item("pistol");
        let attachments = vec![
            att("red_dot", "optic", &["rapid_fire", "heavy"]),
            att("acog", "optic", &["rapid_fire", "heavy"]),
        ];
        try_attach(&mut item, &pistol(), "red_dot", &attachments).unwrap();
        let result = try_attach(&mut item, &pistol(), "acog", &attachments);
        assert_eq!(
            result,
            Err(AttachError::SlotAlreadyOccupied {
                slot_type: "optic".into(),
                current: "red_dot".into(),
            })
        );
        // Original attachment still in place; second wasn't pushed.
        assert_eq!(item.attached, vec!["red_dot".to_string()]);
    }

    #[test]
    fn multiple_different_slots_compose() {
        let mut item = empty_item("pistol");
        let attachments = vec![
            att("red_dot", "optic", &["rapid_fire", "heavy"]),
            att("extended_mag", "magazine", &["rapid_fire"]),
        ];
        try_attach(&mut item, &pistol(), "red_dot", &attachments).unwrap();
        try_attach(&mut item, &pistol(), "extended_mag", &attachments).unwrap();
        assert_eq!(
            item.attached,
            vec!["red_dot".to_string(), "extended_mag".to_string()]
        );
    }

}
