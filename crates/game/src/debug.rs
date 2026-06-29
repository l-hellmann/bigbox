//! Debug / tuning overlay — compiled only under the `debug` feature.
//!
//! An immediate-mode egui panel bound directly to `World::tunables` plus the
//! `World::debug_*` spawn helpers. Every widget here edits live game state; the
//! headless `h2b_game` library knows nothing about it. Toggle with **F1**.
//!
//! Run it with:
//! ```text
//! cargo run -p head2box-game --features debug
//! ```

use egui_macroquad::egui;
use h2b_core::{
    BaseItem, HitResult, ItemInstance, Rarity, Weapon, aggregate_item, dps_against, time_to_kill,
};
use h2b_game::{Content, Tunables, World};
use h2b_procgen::{ArenaParams, Map, MapParams, generate_arena, generate_bsp};
use macroquad::prelude::*;

/// Where tunables export/import. Lives in the process working directory so the
/// file sits next to wherever `cargo run` was launched — easy to find, edit by
/// hand, and check into a presets folder.
const TUNABLES_PATH: &str = "head2box-tunables.ron";

pub struct DebugUi {
    visible: bool,
    archetype: usize,
    spawn_count: i32,
    spawn_distance: u32,
    spawn_at_cursor: bool,
    /// Selected weapon — index into [`Content::bases`] (weapon-slot only).
    weapon_base: usize,
    /// Arena pillar geometry for the next "load arena".
    arena_pillars: bool,
    /// Draw the flow-field next-step arrows (read by the renderer).
    show_flow: bool,
    /// Last export/import outcome, shown under the buttons.
    status: String,
}

impl Default for DebugUi {
    fn default() -> Self {
        Self::new()
    }
}

impl DebugUi {
    pub fn new() -> Self {
        Self {
            visible: true,
            archetype: 0,
            spawn_count: 4,
            spawn_distance: 12,
            spawn_at_cursor: false,
            weapon_base: 0,
            arena_pillars: true,
            show_flow: false,
            status: String::new(),
        }
    }

    /// F1 shows/hides the panel.
    pub fn handle_toggle(&mut self) {
        if is_key_pressed(KeyCode::F1) {
            self.visible = !self.visible;
        }
    }

    /// Whether the renderer should draw the flow-field arrows this frame.
    pub fn show_flow(&self) -> bool {
        self.visible && self.show_flow
    }

    /// Build and apply the panel for this frame. Returns `true` while egui is
    /// capturing the pointer, so the caller can suppress firing/aiming clicks
    /// that land on the panel. `cursor_tile` is the ground position under the
    /// mouse (tile coords), used by the "spawn at cursor" action.
    pub fn run(
        &mut self,
        world: &mut World,
        content: &Content,
        cursor_tile: Option<(f32, f32)>,
    ) -> bool {
        if !self.visible {
            return false;
        }
        let mut wants_pointer = false;
        egui_macroquad::ui(|ctx| {
            wants_pointer = ctx.wants_pointer_input();
            // Pinned to the right edge, full window height. `resizable` lets you
            // widen it; the scroll area handles the tall widget stack.
            egui::SidePanel::right("debug_panel")
                .default_width(300.0)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.heading("debug · tuning  (F1)");
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        self.contents(ui, world, content, cursor_tile);
                    });
                });
        });
        wants_pointer
    }

    /// Paint the panel. Separate from [`run`] so the caller controls draw
    /// ordering (egui goes on top, after the 2D HUD).
    pub fn draw(&self) {
        if self.visible {
            egui_macroquad::draw();
        }
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        world: &mut World,
        content: &Content,
        cursor_tile: Option<(f32, f32)>,
    ) {
        ui.collapsing("level / map", |ui| {
            ui.checkbox(&mut self.arena_pillars, "arena pillars (pathing obstacles)");
            ui.horizontal(|ui| {
                if ui.button("load arena").clicked() {
                    let map = generate_arena(&ArenaParams {
                        pillars: self.arena_pillars,
                        ..Default::default()
                    });
                    reload_world(world, map);
                    // Controlled-testing level: suspend waves, spawn by hand.
                    world.tunables.auto_spawn = false;
                }
                if ui.button("load BSP").clicked() {
                    let map = generate_bsp(&MapParams {
                        seed: 42,
                        ..Default::default()
                    });
                    reload_world(world, map);
                }
            });
            ui.checkbox(&mut self.show_flow, "show flow field (enemy pathing)");
        });

        ui.collapsing("combat", |ui| {
            let t = &mut world.tunables;
            ui.add(egui::Slider::new(&mut t.bullet_damage, 0.0..=200.0).text("bullet dmg"));
            ui.add(egui::Slider::new(&mut t.fire_rate, 0.5..=30.0).text("fire rate /s"));
            ui.add(egui::Slider::new(&mut t.projectile_speed, 4.0..=80.0).text("proj speed"));
            ui.add(egui::Slider::new(&mut t.bullet_lifetime, 0.2..=5.0).text("bullet life s"));
            ui.add(egui::Slider::new(&mut t.crit_chance, 0.0..=1.0).text("crit chance"));
            ui.add(egui::Slider::new(&mut t.crit_multiplier, 1.0..=5.0).text("crit mult"));
            ui.add(egui::Slider::new(&mut t.contact_dps, 0.0..=100.0).text("contact dps"));
        });

        ui.collapsing("movement / pathing", |ui| {
            let t = &mut world.tunables;
            ui.add(egui::Slider::new(&mut t.player_speed, 1.0..=20.0).text("player speed"));
            ui.add(egui::Slider::new(&mut t.enemy_speed_mult, 0.0..=4.0).text("enemy speed ×"));
        });

        ui.collapsing("spawning / loot", |ui| {
            let t = &mut world.tunables;
            ui.checkbox(&mut t.auto_spawn, "auto-spawn waves");
            ui.add(egui::Slider::new(&mut t.spawn_interval, 0.2..=15.0).text("interval s"));
            ui.add(egui::Slider::new(&mut t.spawn_batch, 1..=20).text("batch"));
            ui.add(egui::Slider::new(&mut t.max_enemies, 1..=200).text("max enemies"));
            ui.add(egui::Slider::new(&mut t.spawn_min_distance, 1..=40).text("min distance"));
            ui.add(egui::Slider::new(&mut t.drop_chance, 0.0..=1.0).text("drop chance"));
        });

        ui.checkbox(&mut world.tunables.god_mode, "god mode (no contact damage)");

        ui.separator();
        ui.strong("manual spawn");
        let current = content
            .enemies
            .get(self.archetype)
            .map(|e| e.id.as_str())
            .unwrap_or("—");
        egui::ComboBox::from_label("archetype")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for (i, e) in content.enemies.iter().enumerate() {
                    ui.selectable_value(&mut self.archetype, i, e.id.as_str());
                }
            });
        ui.add(egui::Slider::new(&mut self.spawn_count, 1..=40).text("count"));
        ui.checkbox(&mut self.spawn_at_cursor, "at cursor (else ring around player)");
        if !self.spawn_at_cursor {
            ui.add(egui::Slider::new(&mut self.spawn_distance, 1..=40).text("ring distance"));
        }
        ui.horizontal(|ui| {
            if ui.button("spawn").clicked() {
                if self.spawn_at_cursor {
                    if let Some((cx, cy)) = cursor_tile {
                        for _ in 0..self.spawn_count {
                            world.debug_spawn_at(self.archetype, cx, cy, content);
                        }
                    }
                } else {
                    world.debug_spawn(
                        self.archetype,
                        self.spawn_count as usize,
                        self.spawn_distance,
                        content,
                    );
                }
            }
            if ui.button("clear enemies").clicked() {
                world.debug_clear_enemies();
            }
        });

        ui.separator();
        ui.strong("weapon / TTK");
        // Weapon picker — weapon-slot bases only.
        let weapons: Vec<usize> = content
            .bases
            .iter()
            .enumerate()
            .filter(|(_, b)| b.slot == "weapon")
            .map(|(i, _)| i)
            .collect();
        if !weapons.contains(&self.weapon_base) {
            self.weapon_base = weapons.first().copied().unwrap_or(0);
        }
        let current_weapon = content
            .bases
            .get(self.weapon_base)
            .map(|b| b.name.as_str())
            .unwrap_or("—");
        egui::ComboBox::from_label("weapon")
            .selected_text(current_weapon)
            .show_ui(ui, |ui| {
                for &i in &weapons {
                    ui.selectable_value(&mut self.weapon_base, i, content.bases[i].name.as_str());
                }
            });
        if ui.button("equip selected (load stats → tunables)").clicked()
            && let Some(base) = content.bases.get(self.weapon_base)
        {
            let w = weapon_from_base(base);
            world.tunables.bullet_damage = w.damage_per_shot;
            if w.fire_rate > 0.0 {
                world.tunables.fire_rate = w.fire_rate;
            }
            world.tunables.crit_chance = w.crit_chance;
            world.tunables.crit_multiplier = w.crit_multiplier;
        }
        // Live expected TTK/DPS of the *current* tunables weapon against the
        // enemy archetype selected above — the core expected-value lens, the
        // same numbers the sim reports. Retune the sliders and watch it move.
        let weapon = Weapon {
            damage_per_shot: world.tunables.bullet_damage,
            fire_rate: world.tunables.fire_rate,
            crit_chance: world.tunables.crit_chance,
            crit_multiplier: world.tunables.crit_multiplier,
        };
        if let Some(enemy) = content.enemies.get(self.archetype) {
            let target = enemy.as_combatant();
            let dps = dps_against(&weapon, &target);
            let ttk = time_to_kill(&weapon, &target)
                .map(|s| format!("{s:.2}s"))
                .unwrap_or_else(|| "∞".to_string());
            ui.label(format!(
                "vs {} (life {:.0}, armor {:.0}, eva {:.0}):",
                enemy.id, target.current_life, target.armor, target.evasion
            ));
            ui.label(format!("  expected dps {dps:.1}   ttk {ttk}"));
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("revive / heal").clicked() {
                world.debug_revive();
            }
            if ui.button("clear drops").clicked() {
                world.debug_clear_drops();
            }
            if ui.button("reset tunables").clicked() {
                world.tunables = Tunables::default();
            }
        });

        ui.separator();
        ui.strong("tunables file");
        ui.horizontal(|ui| {
            if ui.button("export").clicked() {
                self.status = match export(&world.tunables) {
                    Ok(path) => format!("exported → {path}"),
                    Err(e) => format!("export failed: {e}"),
                };
            }
            if ui.button("import").clicked() {
                self.status = match import() {
                    Ok(t) => {
                        world.tunables = t;
                        format!("imported ← {TUNABLES_PATH}")
                    }
                    Err(e) => format!("import failed: {e}"),
                };
            }
        });
        if !self.status.is_empty() {
            ui.label(&self.status);
        }

        ui.separator();
        ui.label(format!("fps {}", get_fps()));
        ui.label(format!(
            "enemies {}   projectiles {}   drops {}",
            world.enemies.len(),
            world.projectiles.len(),
            world.drops.len(),
        ));
        ui.label(format!(
            "player tile ({}, {})",
            world.player.x as u32, world.player.y as u32
        ));
        let last = match &world.last_hit {
            Some(HitResult::Hit {
                damage_dealt,
                was_crit,
            }) => format!("last hit {:.1}{}", damage_dealt, if *was_crit { " CRIT" } else { "" }),
            Some(HitResult::Dodged) => "last hit DODGED".to_string(),
            None => "last hit —".to_string(),
        };
        ui.label(last);
    }
}

/// Swap the world onto a new map, preserving the current tunables (a `World`
/// is otherwise rebuilt from scratch with defaults). Inventory / kills / XP
/// reset — this is a fresh level, not a checkpoint.
fn reload_world(world: &mut World, map: Map) {
    let saved = world.tunables;
    *world = World::new(map);
    world.tunables = saved;
}

/// Build a combat [`Weapon`] from a weapon base's intrinsic stats — a tier-0,
/// zero-affix, no-attachment instance run through the canonical
/// `aggregate_item` → `Weapon::from_stats` path (same as the sim), so the
/// debug numbers match the balance tool's.
fn weapon_from_base(base: &BaseItem) -> Weapon {
    let item = ItemInstance {
        base: base.id.clone(),
        ilvl: 1,
        rarity: Rarity::Basic,
        seed: 0,
        prefixes: vec![],
        suffixes: vec![],
        upgrade_tier: 0,
        attached: vec![],
    };
    let stats = aggregate_item(&item, base, &[], &[]);
    Weapon::from_stats(&stats)
}

/// Write the current tunables to [`TUNABLES_PATH`] as pretty RON (the project's
/// content format, so the file is hand-editable). Returns the absolute path on
/// success for the status line.
fn export(tunables: &Tunables) -> Result<String, String> {
    let ron = ron::ser::to_string_pretty(tunables, ron::ser::PrettyConfig::default())
        .map_err(|e| e.to_string())?;
    std::fs::write(TUNABLES_PATH, ron).map_err(|e| e.to_string())?;
    let abs = std::env::current_dir()
        .map(|d| d.join(TUNABLES_PATH).display().to_string())
        .unwrap_or_else(|_| TUNABLES_PATH.to_string());
    Ok(abs)
}

/// Read tunables back from [`TUNABLES_PATH`]. A malformed or stale file is a
/// recoverable error surfaced in the status line, not a panic.
fn import() -> Result<Tunables, String> {
    let text = std::fs::read_to_string(TUNABLES_PATH).map_err(|e| e.to_string())?;
    ron::from_str(&text).map_err(|e| e.to_string())
}
