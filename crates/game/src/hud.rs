//! 2D UI (bevy_ui port of the macroquad `render.rs::draw_hud` + `ui.rs`): the
//! always-on HUD and the inventory modal.
//!
//! The HUD is a persistent node tree spawned once; a per-frame system mutates
//! the `Text`/`Node` of its dynamic pieces (no tree rebuild). The inventory
//! modal is spawned on open and despawned on close/rebuild; clicks drive the
//! same `World` methods the old immediate-mode `InvAction` did.

use crate::scene::rarity_color;
use crate::{GameContent, Sim};
use bb_core::progression::level_for_total_xp;
use bb_game::FireProfile;
use bevy::prelude::*;

// ---- theme ----
const PANEL_BG: Color = Color::srgb(0.06, 0.07, 0.09);
const PANEL_BORDER: Color = Color::srgb(0.22, 0.26, 0.32);
const SLOT_BG: Color = Color::srgb(0.11, 0.13, 0.16);
const SLOT_HOVER: Color = Color::srgb(0.18, 0.22, 0.27);
const TEXT: Color = Color::srgb(0.88, 0.90, 0.92);
const TEXT_DIM: Color = Color::srgb(0.55, 0.58, 0.62);
const ACTIVE_BORDER: Color = Color::srgb(0.55, 0.95, 0.60);
const SCREEN_DIM: Color = Color::srgba(0.0, 0.0, 0.0, 0.55);

/// The bundled UI font (Quantico, SIL OFL 1.1 — `assets/fonts/quantico/OFL.txt`),
/// embedded so the shipped binary stays self-contained (no runtime file read).
#[derive(Resource)]
pub struct UiFont(Handle<Font>);

impl UiFont {
    fn text(&self, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
        (
            Text::new(s),
            TextFont {
                font: self.0.clone().into(),
                font_size: size.into(),
                ..default()
            },
            TextColor(color),
        )
    }
}

/// Whether the inventory modal is open (pauses the sim). Toggled in `ui_input`.
#[derive(Resource, Default)]
pub struct InventoryOpen(pub bool);

// ---- HUD markers ----
// `pub(crate)` because the (crate-registered) systems' query filters mention
// them — a `pub(crate)` system can't expose module-private types.
#[derive(Component)]
pub(crate) struct HealthText;
#[derive(Component)]
pub(crate) struct HealthFill;
#[derive(Component)]
pub(crate) struct StatsText;
#[derive(Component)]
pub(crate) struct WeaponText;
#[derive(Component)]
pub(crate) struct WheelText;
#[derive(Component)]
pub(crate) struct PickupText;
#[derive(Component)]
pub(crate) struct DeathRoot;

// ---- Startup: font + HUD tree ----

/// Load the embedded font and build the persistent HUD node tree.
pub(crate) fn setup_ui(mut commands: Commands, mut fonts: ResMut<Assets<Font>>) {
    let handle = fonts.add(Font::from_bytes(
        include_bytes!("../assets/fonts/quantico/Quantico-Regular.ttf").to_vec(),
    ));
    let font = UiFont(handle);

    // Top-left stat stack.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(12.0),
                left: Val::Px(12.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                ..default()
            },
        ))
        .with_children(|c| {
            // Health bar: dark backing + red fill + centered numeric text.
            c.spawn((
                Node {
                    width: Val::Px(260.0),
                    height: Val::Px(22.0),
                    border: UiRect::all(Val::Px(2.0)),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                BackgroundColor(Color::srgba(0.15, 0.05, 0.05, 0.9)),
                BorderColor::all(Color::BLACK),
            ))
            .with_children(|bar| {
                // Fill sits behind the text, pinned to the left, width = life%.
                bar.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        top: Val::Px(0.0),
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.85, 0.20, 0.20)),
                    HealthFill,
                ));
                bar.spawn((font.text("", 18.0, Color::WHITE), HealthText));
            });

            c.spawn((font.text("", 20.0, Color::srgb(0.85, 0.85, 0.85)), StatsText));
            c.spawn((font.text("", 17.0, Color::srgb(0.80, 0.80, 0.62)), WeaponText));
            c.spawn((font.text("", 16.0, Color::srgb(0.62, 0.66, 0.55)), WheelText));
            c.spawn((font.text("", 17.0, Color::srgb(0.7, 0.85, 0.7)), PickupText));
        });

    // Controls hint, bottom-left.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(10.0),
            left: Val::Px(12.0),
            ..default()
        },
        font.text(
            "WASD move  |  aim  |  LMB / Space shoot  |  1-4 / wheel weapon  |  I inventory  |  ESC quit",
            16.0,
            Color::srgb(0.55, 0.55, 0.55),
        ),
    ));

    // Death overlay (centered), hidden until game over.
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                row_gap: Val::Px(16.0),
                ..default()
            },
            Visibility::Hidden,
            DeathRoot,
        ))
        .with_children(|c| {
            c.spawn(font.text("YOU DIED", 64.0, Color::srgb(0.9, 0.1, 0.1)));
            c.spawn(font.text("press R to restart", 28.0, Color::WHITE));
        });

    commands.insert_resource(font);
}

// ---- Per-frame: HUD update ----

/// Refresh the HUD's dynamic text + health fill from the (possibly frozen)
/// world. Mutates existing components — no tree rebuild.
#[allow(clippy::type_complexity)]
pub(crate) fn hud_update(
    sim: Res<Sim>,
    mut texts: ParamSet<(
        Query<&mut Text, With<StatsText>>,
        Query<&mut Text, With<WeaponText>>,
        Query<&mut Text, With<WheelText>>,
        Query<&mut Text, With<PickupText>>,
        Query<&mut Text, With<HealthText>>,
    )>,
    mut fill: Query<&mut Node, With<HealthFill>>,
    mut death: Query<&mut Visibility, With<DeathRoot>>,
) {
    let w = &sim.0;
    let p = &w.player;

    let frac = (p.current_life / p.max_life).clamp(0.0, 1.0);
    if let Ok(mut n) = fill.single_mut() {
        n.width = Val::Percent(frac * 100.0);
    }
    if let Ok(mut t) = texts.p4().single_mut() {
        t.0 = format!("{:.0} / {:.0}", p.current_life, p.max_life);
    }

    let level = level_for_total_xp(w.xp);
    if let Ok(mut t) = texts.p0().single_mut() {
        t.0 = format!(
            "Lv {level}   kills {}   xp {}   enemies {}   loot {}",
            w.kills,
            w.xp,
            w.enemies.len(),
            w.inventory.len(),
        );
    }

    // Equipped-weapon line + weapon-wheel line.
    let (weapon_line, wheel_line) = match w.equipped() {
        Some(eq) => {
            let tag = match eq.profile {
                FireProfile::Spread { .. } => {
                    format!("   {}-pellet spread", w.tunables.spread_pellets)
                }
                FireProfile::Explosive { .. } => {
                    format!("   blast r{:.1}", w.tunables.blast_radius)
                }
                FireProfile::Single => String::new(),
            };
            let weapon = format!(
                "weapon: {}   dmg {:.0}   rate {:.1}/s{tag}",
                eq.name, eq.weapon.damage_per_shot, eq.weapon.fire_rate
            );
            let rack = w.loadout();
            let wheel = if rack.len() > 1 {
                rack.iter()
                    .enumerate()
                    .map(|(i, wp)| {
                        let mark = if i == w.active_slot() { "*" } else { "" };
                        format!("{}:{}{mark}", i + 1, wp.name)
                    })
                    .collect::<Vec<_>>()
                    .join("  ")
            } else {
                String::new()
            };
            (weapon, wheel)
        }
        None => (String::new(), String::new()),
    };
    if let Ok(mut t) = texts.p1().single_mut() {
        t.0 = weapon_line;
    }
    if let Ok(mut t) = texts.p2().single_mut() {
        t.0 = wheel_line;
    }

    if let Ok(mut t) = texts.p3().single_mut() {
        t.0 = match &w.last_pickup {
            Some(last) => format!("picked up: {last}"),
            None => String::new(),
        };
    }

    if let Ok(mut vis) = death.single_mut() {
        *vis = if w.game_over {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

// ---- Inventory modal ----

/// Root of the spawned inventory modal (present only while open).
#[derive(Component)]
pub(crate) struct InventoryRoot;

/// What an inventory button does when clicked — mirrors the old `InvAction`.
#[derive(Component, Clone, Copy)]
pub(crate) enum InvButton {
    Close,
    Switch(usize),
    Equip(usize),
}

/// Spawn the modal when it opens; despawn it when it closes. A successful
/// switch/equip despawns the root (see `inventory_clicks`) so this rebuilds it
/// next frame with fresh rack/bag contents.
pub(crate) fn manage_inventory(
    mut commands: Commands,
    open: Res<InventoryOpen>,
    font: Res<UiFont>,
    sim: Res<Sim>,
    content: Res<GameContent>,
    root: Query<Entity, With<InventoryRoot>>,
) {
    match (open.0, root.single()) {
        (true, Err(_)) => spawn_inventory(&mut commands, &font, &sim.0, &content.0),
        (false, Ok(e)) => commands.entity(e).despawn(),
        _ => {}
    }
}

fn spawn_inventory(commands: &mut Commands, font: &UiFont, world: &bb_game::World, content: &bb_game::Content) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(0.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(SCREEN_DIM),
            GlobalZIndex(10),
            InventoryRoot,
        ))
        .with_children(|overlay| {
            overlay
                .spawn((
                    Node {
                        width: Val::Px(700.0),
                        max_width: Val::Percent(95.0),
                        height: Val::Px(500.0),
                        max_height: Val::Percent(95.0),
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::all(Val::Px(20.0)),
                        row_gap: Val::Px(10.0),
                        border: UiRect::all(Val::Px(2.0)),
                        ..default()
                    },
                    BackgroundColor(PANEL_BG),
                    BorderColor::all(PANEL_BORDER),
                ))
                .with_children(|panel| {
                    // Title row: heading + close button.
                    panel
                        .spawn(Node {
                            width: Val::Percent(100.0),
                            justify_content: JustifyContent::SpaceBetween,
                            align_items: AlignItems::Center,
                            ..default()
                        })
                        .with_children(|row| {
                            row.spawn(font.text("INVENTORY", 28.0, TEXT));
                            row.spawn((
                                Button,
                                InvButton::Close,
                                Node {
                                    width: Val::Px(30.0),
                                    height: Val::Px(30.0),
                                    justify_content: JustifyContent::Center,
                                    align_items: AlignItems::Center,
                                    border: UiRect::all(Val::Px(1.5)),
                                    ..default()
                                },
                                BackgroundColor(SLOT_BG),
                                BorderColor::all(PANEL_BORDER),
                            ))
                            .with_child(font.text("x", 18.0, TEXT));
                        });

                    // EQUIPPED rack row.
                    panel.spawn(font.text("EQUIPPED", 16.0, TEXT_DIM));
                    panel
                        .spawn(Node {
                            column_gap: Val::Px(10.0),
                            ..default()
                        })
                        .with_children(|rack| {
                            for (i, wp) in world.loadout().iter().enumerate() {
                                let active = i == world.active_slot();
                                let border = if active {
                                    ACTIVE_BORDER
                                } else {
                                    rarity_color(wp.item.rarity)
                                };
                                rack.spawn((
                                    Button,
                                    InvButton::Switch(i),
                                    Node {
                                        width: Val::Px(156.0),
                                        height: Val::Px(46.0),
                                        flex_direction: FlexDirection::Column,
                                        justify_content: JustifyContent::Center,
                                        padding: UiRect::all(Val::Px(8.0)),
                                        border: UiRect::all(Val::Px(if active { 2.5 } else { 1.5 })),
                                        ..default()
                                    },
                                    BackgroundColor(SLOT_BG),
                                    BorderColor::all(border),
                                ))
                                .with_children(|slot| {
                                    slot.spawn(font.text(format!("{}: {}", i + 1, wp.name), 16.0, TEXT));
                                    slot.spawn(font.text(
                                        format!(
                                            "dmg {:.0}  rate {:.1}",
                                            wp.weapon.damage_per_shot, wp.weapon.fire_rate
                                        ),
                                        13.0,
                                        TEXT_DIM,
                                    ));
                                });
                            }
                        });

                    // Divider + BAG heading.
                    panel.spawn((
                        Node {
                            width: Val::Percent(100.0),
                            height: Val::Px(1.0),
                            margin: UiRect::vertical(Val::Px(6.0)),
                            ..default()
                        },
                        BackgroundColor(PANEL_BORDER),
                    ));
                    panel.spawn(font.text("BAG", 16.0, TEXT_DIM));

                    if world.inventory.is_empty() {
                        panel.spawn(font.text(
                            "(empty — walk over drops to collect them)",
                            16.0,
                            TEXT_DIM,
                        ));
                    }

                    // Bag grid: wrapping row of item cells.
                    panel
                        .spawn(Node {
                            width: Val::Percent(100.0),
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: Val::Px(10.0),
                            row_gap: Val::Px(10.0),
                            ..default()
                        })
                        .with_children(|grid| {
                            for (i, item) in world.inventory.iter().enumerate() {
                                let base = content.bases.iter().find(|b| b.id == item.base);
                                let is_weapon = base.map(|b| b.slot == "weapon").unwrap_or(false);
                                let name = base.map(|b| b.name.as_str()).unwrap_or(item.base.as_str());
                                let sub = if is_weapon { "click to equip" } else { "armor" };
                                let name_color = if is_weapon { TEXT } else { TEXT_DIM };

                                let mut cell = grid.spawn((
                                    Node {
                                        width: Val::Px(150.0),
                                        height: Val::Px(56.0),
                                        flex_direction: FlexDirection::Column,
                                        justify_content: JustifyContent::Center,
                                        padding: UiRect::all(Val::Px(8.0)),
                                        border: UiRect::all(Val::Px(1.5)),
                                        ..default()
                                    },
                                    BackgroundColor(SLOT_BG),
                                    BorderColor::all(rarity_color(item.rarity)),
                                ));
                                // Only weapons are clickable (equip); armor is inert.
                                if is_weapon {
                                    cell.insert((Button, InvButton::Equip(i)));
                                }
                                cell.with_children(|c| {
                                    c.spawn(font.text(name, 17.0, name_color));
                                    c.spawn(font.text(format!("{:?} · {sub}", item.rarity), 13.0, TEXT_DIM));
                                });
                            }
                        });

                    panel.spawn(font.text(
                        "click a weapon to equip  ·  I / Tab / Esc to close",
                        14.0,
                        TEXT_DIM,
                    ));
                });
        });
}

/// Handle inventory button clicks — the same `World` mutations the old
/// `InvAction` did. Switch/equip despawn the modal root so `manage_inventory`
/// rebuilds it with fresh contents next frame.
pub(crate) fn inventory_clicks(
    mut commands: Commands,
    mut sim: ResMut<Sim>,
    content: Res<GameContent>,
    mut open: ResMut<InventoryOpen>,
    buttons: Query<(&Interaction, &InvButton), Changed<Interaction>>,
    root: Query<Entity, With<InventoryRoot>>,
) {
    for (interaction, button) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *button {
            InvButton::Close => open.0 = false,
            InvButton::Switch(slot) => {
                sim.0.switch_weapon(slot);
                if let Ok(e) = root.single() {
                    commands.entity(e).despawn();
                }
            }
            InvButton::Equip(index) => {
                sim.0.equip_from_inventory(index, &content.0);
                if let Ok(e) = root.single() {
                    commands.entity(e).despawn();
                }
            }
        }
    }
}

/// Hover-light inventory buttons (SLOT_BG ↔ SLOT_HOVER).
pub(crate) fn inventory_hover(
    mut buttons: Query<(&Interaction, &mut BackgroundColor), (Changed<Interaction>, With<InvButton>)>,
) {
    for (interaction, mut bg) in &mut buttons {
        bg.0 = match interaction {
            Interaction::Hovered | Interaction::Pressed => SLOT_HOVER,
            Interaction::None => SLOT_BG,
        };
    }
}
