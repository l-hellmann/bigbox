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
use h2b_core::HitResult;
use h2b_game::{Content, Tunables, World};
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
            status: String::new(),
        }
    }

    /// F1 shows/hides the panel.
    pub fn handle_toggle(&mut self) {
        if is_key_pressed(KeyCode::F1) {
            self.visible = !self.visible;
        }
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
            egui::Window::new("debug · tuning  (F1)")
                .default_width(300.0)
                .show(ctx, |ui| self.contents(ui, world, content, cursor_tile));
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
        ui.collapsing("combat", |ui| {
            let t = &mut world.tunables;
            ui.add(egui::Slider::new(&mut t.bullet_damage, 0.0..=200.0).text("bullet dmg"));
            ui.add(egui::Slider::new(&mut t.fire_rate, 0.5..=30.0).text("fire rate /s"));
            ui.add(egui::Slider::new(&mut t.projectile_speed, 4.0..=80.0).text("proj speed"));
            ui.add(egui::Slider::new(&mut t.bullet_lifetime, 0.2..=5.0).text("bullet life s"));
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
