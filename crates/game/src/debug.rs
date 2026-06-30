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
use h2b_core::{HitResult, Weapon, dps_against, time_to_kill};
use h2b_game::{Content, Tunables, World};
use h2b_procgen::{ArenaParams, Map, MapParams, generate_arena, generate_bsp};
use macroquad::prelude::*;

/// Where tunables export/import. Lives in the process working directory so the
/// file sits next to wherever `cargo run` was launched — easy to find, edit by
/// hand, and check into a presets folder.
const TUNABLES_PATH: &str = "head2box-tunables.ron";

/// Accent used for the flat-section headers — the visual marker that a section
/// is pinned/always-on rather than folded behind a triangle.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x3a, 0x7a, 0xc8);

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
    /// Draw floating Enemy/Combatant + player stat blocks (read by the renderer).
    show_entity_stats: bool,
    /// Panel layout: `true` = docked to the right edge, `false` = a free-floating
    /// movable window. Toggled by the checkbox in the panel header.
    docked: bool,
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
            show_entity_stats: false,
            docked: true,
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

    /// Whether the renderer should draw floating entity stat blocks this frame.
    pub fn show_entity_stats(&self) -> bool {
        self.visible && self.show_entity_stats
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
        pad_diag: &crate::PadDiag,
    ) -> bool {
        if !self.visible {
            return false;
        }
        // egui-miniquad only propagates the native DPI to egui on a *change*
        // after startup, not on the first frame — so with `high_dpi` the panel
        // would render at half size (1.0 pixels-per-point in a 2× framebuffer).
        // Pin it to the display scale ourselves. `set_pixels_per_point` targets
        // an absolute value via the zoom factor, so it stays correct even if
        // egui-miniquad later sets the native scale on a monitor change.
        let dpi = screen_dpi_scale();
        egui_macroquad::cfg(|ctx| {
            ctx.set_pixels_per_point(dpi);
            // Forgive cursor jitter between press and release: egui otherwise
            // reclassifies a click that drifts more than `max_click_dist` (6pt
            // default) as a *drag* and drops it — the main cause of "missed"
            // clicks on a trackpad / high-DPI display. Widen the tolerance.
            ctx.options_mut(|o| o.input_options.max_click_dist = 14.0);
        });

        let mut wants_pointer = false;
        let docked = self.docked;
        egui_macroquad::ui(|ctx| {
            if docked {
                // Pinned to the right edge, full window height. `resizable` lets
                // you widen it.
                egui::SidePanel::right("debug_panel")
                    .default_width(300.0)
                    .resizable(true)
                    .show(ctx, |ui| {
                        self.panel_body(ui, world, content, cursor_tile, pad_diag)
                    });
            } else {
                // Free-floating, movable window.
                egui::Window::new("debug · tuning")
                    .default_width(300.0)
                    .default_pos([20.0, 20.0])
                    .resizable(true)
                    .show(ctx, |ui| {
                        self.panel_body(ui, world, content, cursor_tile, pad_diag)
                    });
            }
            // Read after the panel is built so it reflects this frame's
            // interaction (used to suppress firing through the panel).
            wants_pointer = ctx.wants_pointer_input();
        });
        wants_pointer
    }

    /// Shared panel contents for both docked and floating modes: header with the
    /// dock toggle, then the scrolling widget stack.
    fn panel_body(
        &mut self,
        ui: &mut egui::Ui,
        world: &mut World,
        content: &Content,
        cursor_tile: Option<(f32, f32)>,
        pad_diag: &crate::PadDiag,
    ) {
        ui.horizontal(|ui| {
            ui.heading("debug · tuning  (F1)");
            ui.checkbox(&mut self.docked, "docked");
        });
        // `drag_to_scroll(false)`: otherwise a click with any slight cursor
        // movement (common on a trackpad) is taken as a scroll-drag and the
        // button click is dropped.
        egui::ScrollArea::vertical()
            .drag_to_scroll(false)
            .show(ui, |ui| {
                self.contents(ui, world, content, cursor_tile, pad_diag);
            });
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
        pad_diag: &crate::PadDiag,
    ) {
        // Pinned at the very top: god mode is toggled constantly while testing,
        // so it never hides inside a fold.
        ui.add_space(2.0);
        ui.checkbox(&mut world.tunables.god_mode, "god mode  ·  no contact damage");

        // Flat, always-on sections — the controls touched every session.
        self.loadout_section(ui, world, content);
        self.spawning_section(ui, world, content, cursor_tile);

        // Folded, set-and-forget sections (collapsed by default).
        ui.add_space(8.0);
        egui::CollapsingHeader::new("movement / pathing")
            .show(ui, |ui| Self::movement_body(ui, world));
        egui::CollapsingHeader::new("level / map")
            .show(ui, |ui| self.level_body(ui, world));
        controller_section(ui, pad_diag);
        egui::CollapsingHeader::new("world actions")
            .show(ui, |ui| Self::actions_body(ui, world));
        egui::CollapsingHeader::new("tunables file")
            .show(ui, |ui| self.file_body(ui, world));

        Self::stats_footer(ui, world);
    }

    /// Flat: weapon picker + combat sliders + live TTK as one block. Equipping a
    /// weapon loads its stats into these very sliders, and the readout reflects
    /// them against the manual-spawn archetype — equip, tune, watch TTK move.
    fn loadout_section(&mut self, ui: &mut egui::Ui, world: &mut World, content: &Content) {
        flat_header(ui, "Weapon & Combat", "loadout · feel");

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
        if ui.button("equip selected · arm the player").clicked()
            && let Some(base) = content.bases.get(self.weapon_base)
        {
            // Routes through the real loadout path: sets `world.equipped` and
            // loads the weapon's stats into these very sliders (the fire path's
            // read surface), so equip → tune → watch TTK move still works.
            world.equip_base(&base.id, content);
        }

        ui.add_space(4.0);
        let t = &mut world.tunables;
        ui.add(egui::Slider::new(&mut t.bullet_damage, 0.0..=200.0).text("bullet dmg"));
        ui.add(egui::Slider::new(&mut t.fire_rate, 0.5..=30.0).text("fire rate /s"));
        ui.add(egui::Slider::new(&mut t.projectile_speed, 4.0..=80.0).text("proj speed"));
        ui.add(egui::Slider::new(&mut t.bullet_lifetime, 0.2..=5.0).text("bullet life s"));
        ui.add(egui::Slider::new(&mut t.crit_chance, 0.0..=1.0).text("crit chance"));
        ui.add(egui::Slider::new(&mut t.crit_multiplier, 1.0..=5.0).text("crit mult"));
        ui.add(egui::Slider::new(&mut t.contact_dps, 0.0..=100.0).text("contact dps"));

        // Live expected DPS/TTK of the current tunables weapon against the
        // selected archetype — the same expected-value lens the sim reports.
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
            egui::Frame::default()
                .fill(egui::Color32::from_rgb(0x1c, 0x21, 0x28))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(0x2c, 0x3b, 0x4c)))
                .inner_margin(egui::Margin { left: 10, right: 10, top: 7, bottom: 7 })
                .outer_margin(egui::Margin { left: 0, right: 0, top: 8, bottom: 2 })
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "vs {} · life {:.0}, armor {:.0}, eva {:.0}",
                            enemy.id, target.current_life, target.armor, target.evasion
                        ))
                        .weak()
                        .size(11.5),
                    );
                    ui.horizontal(|ui| {
                        ui.label("expected dps");
                        ui.label(
                            egui::RichText::new(format!("{dps:.1}"))
                                .strong()
                                .color(egui::Color32::from_rgb(0x84, 0xd1, 0x8a)),
                        );
                        ui.label("·  ttk");
                        ui.label(
                            egui::RichText::new(ttk)
                                .strong()
                                .color(egui::Color32::from_rgb(0xe3, 0xc7, 0x5a)),
                        );
                    });
                });
        }
    }

    /// Flat: auto-wave cadence and manual spawning together — the same job. The
    /// archetype chosen here also drives the TTK readout in the loadout section.
    fn spawning_section(
        &mut self,
        ui: &mut egui::Ui,
        world: &mut World,
        content: &Content,
        cursor_tile: Option<(f32, f32)>,
    ) {
        flat_header(ui, "Spawning", "auto · manual");

        let t = &mut world.tunables;
        ui.checkbox(&mut t.auto_spawn, "auto-spawn waves");
        ui.add(egui::Slider::new(&mut t.spawn_interval, 0.2..=15.0).text("interval s"));
        ui.add(egui::Slider::new(&mut t.spawn_batch, 1..=20).text("batch"));
        ui.add(egui::Slider::new(&mut t.max_enemies, 1..=200).text("max enemies"));
        ui.add(egui::Slider::new(&mut t.spawn_min_distance, 1..=40).text("min distance"));
        ui.add(egui::Slider::new(&mut t.drop_chance, 0.0..=1.0).text("drop chance"));

        subhead(ui, "manual");
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
            if ui.button("wake all").clicked() {
                world.debug_wake_all();
            }
        });
    }

    /// Folded: arena/BSP map swaps and the renderer debug-viz toggles.
    fn level_body(&mut self, ui: &mut egui::Ui, world: &mut World) {
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
        ui.checkbox(&mut self.show_entity_stats, "show entity stats (floating)");
    }

    /// Folded: movement / pathing tunables.
    fn movement_body(ui: &mut egui::Ui, world: &mut World) {
        let t = &mut world.tunables;
        ui.add(egui::Slider::new(&mut t.player_speed, 1.0..=20.0).text("player speed"));
        ui.add(egui::Slider::new(&mut t.enemy_speed_mult, 0.0..=4.0).text("enemy speed ×"));
        ui.add(egui::Slider::new(&mut t.sight_range, 0.0..=40.0).text("sight range (aggro)"));
        ui.add(egui::Slider::new(&mut t.los_range, 0.0..=40.0).text("LOS beeline range"));
        ui.add(egui::Slider::new(&mut t.separation_weight, 0.0..=5.0).text("separation weight"));
        ui.add(egui::Slider::new(&mut t.separation_radius, 0.0..=3.0).text("separation radius"));
        ui.add(egui::Slider::new(&mut t.stick_deadzone, 0.0..=0.6).text("stick deadzone"));
    }

    /// Folded: one-shot world actions.
    fn actions_body(ui: &mut egui::Ui, world: &mut World) {
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
    }

    /// Folded: export / import the tunables preset file.
    fn file_body(&mut self, ui: &mut egui::Ui, world: &mut World) {
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
    }

    /// Always-visible footer: live counts and the last per-shot hit readout.
    fn stats_footer(ui: &mut egui::Ui, world: &World) {
        ui.add_space(6.0);
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

/// Header for a pinned flat section: an accent bar + uppercase title, with an
/// optional right-aligned hint. The visual counterpart to a `CollapsingHeader`'s
/// triangle — "always-on, primary" vs. "folded, secondary".
fn flat_header(ui: &mut egui::Ui, title: &str, hint: &str) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let (bar, _) = ui.allocate_exact_size(egui::vec2(3.0, 15.0), egui::Sense::hover());
        ui.painter().rect_filled(bar, 2.0, ACCENT);
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(title.to_uppercase())
                .strong()
                .size(12.5)
                .color(egui::Color32::from_gray(238)),
        );
        if !hint.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(hint).weak().size(11.0));
            });
        }
    });
    ui.add_space(2.0);
}

/// A minor divider label inside a section (e.g. "manual" within Spawning).
fn subhead(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text.to_uppercase()).weak().size(10.5));
    ui.add_space(1.0);
}

/// Live controller diagnostics — surfaces whether gilrs sees a pad, its SDL
/// mapping (the usual "connected but dead" culprit when missing), and live
/// stick/trigger/button state so you can confirm inputs are reaching the game.
fn controller_section(ui: &mut egui::Ui, diag: &crate::PadDiag) {
    egui::CollapsingHeader::new("controller")
        .default_open(false)
        .show(ui, |ui| {
            if !diag.initialized {
                ui.colored_label(egui::Color32::RED, "gilrs failed to initialize");
                return;
            }
            if diag.pads.is_empty() {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "no gamepad detected — gilrs sees 0 devices",
                );
                ui.label("(on macOS, Xbox controllers over USB use a proprietary");
                ui.label(" protocol IOKit/HID can't read — try Bluetooth pairing)");
                return;
            }
            for (i, p) in diag.pads.iter().enumerate() {
                ui.colored_label(egui::Color32::LIGHT_GREEN, format!("[{i}] {}", p.name));
                let mapped = !p.mapping.starts_with("UNMAPPED");
                ui.colored_label(
                    if mapped { egui::Color32::GRAY } else { egui::Color32::YELLOW },
                    format!("  mapping: {}", p.mapping),
                );
                ui.label(format!("  power: {}", p.power));
                ui.label(format!(
                    "  L-stick: ({:+.2}, {:+.2})",
                    p.left_stick.0, p.left_stick.1
                ));
                ui.label(format!(
                    "  R-stick: ({:+.2}, {:+.2})",
                    p.right_stick.0, p.right_stick.1
                ));
                ui.label(format!("  RT: {:.2}", p.right_trigger));
                let btns = if p.buttons_down.is_empty() {
                    "—".to_string()
                } else {
                    p.buttons_down.join(", ")
                };
                ui.label(format!("  buttons: {btns}"));

                // Raw (pre-mapping) state — the data needed to build a mapping
                // for an unmapped pad. Move each stick/trigger and watch which
                // raw axis code changes; press buttons to see their codes.
                if !mapped {
                    ui.separator();
                    ui.colored_label(egui::Color32::LIGHT_BLUE, "  raw (for mapping):");
                    ui.label(format!("  uuid: {}", p.uuid));
                    for (code, val) in &p.raw_axes {
                        ui.label(format!("    axis {code}: {val:+.2}"));
                    }
                    let rb = if p.raw_buttons.is_empty() {
                        "—".to_string()
                    } else {
                        p.raw_buttons.join(", ")
                    };
                    ui.label(format!("    btn pressed: {rb}"));
                }
            }
        });
}

/// Swap the world onto a new map, preserving the current tunables (a `World`
/// is otherwise rebuilt from scratch with defaults). Inventory / kills / XP
/// reset — this is a fresh level, not a checkpoint.
fn reload_world(world: &mut World, map: Map) {
    let saved = world.tunables;
    *world = World::new(map);
    world.tunables = saved;
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
