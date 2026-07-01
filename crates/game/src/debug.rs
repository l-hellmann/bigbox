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
use h2b_core::{HitResult, Rarity, Weapon, dps_against, time_to_kill};
use h2b_game::{Content, FireProfile, Tunables, World};
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
    /// Loot drop: `0` = random base, else `1 + index` into [`Content::bases`].
    drop_base: usize,
    /// Rarity to force on a debug drop.
    drop_rarity: Rarity,
    /// Item level to roll a debug drop at.
    drop_ilvl: u32,
    /// Drop at the cursor tile (else on the player, for instant pickup).
    drop_at_cursor: bool,
    /// Arena pillar geometry for the next "load arena".
    arena_pillars: bool,
    /// Draw the flow-field next-step arrows (read by the renderer).
    show_flow: bool,
    /// Draw floating Enemy/Combatant + player stat blocks (read by the renderer).
    show_entity_stats: bool,
    /// Which tool windows are open. The launcher's checkboxes and each window's
    /// close [x] both drive these (kept in sync via a local, see [`Self::run`]).
    win_combat: bool,
    win_spawning: bool,
    win_loot: bool,
    win_movement: bool,
    win_level: bool,
    win_controller: bool,
    win_actions: bool,
    win_file: bool,
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
            drop_base: 0,
            drop_rarity: Rarity::Rare,
            drop_ilvl: 20,
            drop_at_cursor: false,
            arena_pillars: true,
            show_flow: false,
            show_entity_stats: false,
            // All tool windows start closed; open them on demand from the
            // launcher checkboxes.
            win_combat: false,
            win_spawning: false,
            win_loot: false,
            win_movement: false,
            win_level: false,
            win_controller: false,
            win_actions: false,
            win_file: false,
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
        egui_macroquad::ui(|ctx| {
            // Launcher: always-on compact window with god mode, the per-tool
            // open toggles, and the live stats footer.
            egui::Window::new("debug (F1)")
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
                .default_width(220.0)
                .show(ctx, |ui| self.launcher_body(ui, world));

            // Tool windows — each renders only when its open bool is set, and its
            // own close [x] clears that bool. `window()` copies the bool to a
            // local for `.open()` so we don't borrow `self` twice. The `slot`
            // index cascades their initial position out from beside the launcher.
            self.window(ctx, 0, "Weapon & Combat", Self::win_combat_get, |s, ui| {
                s.loadout_section(ui, world, content)
            });
            self.window(ctx, 1, "Spawning", Self::win_spawning_get, |s, ui| {
                s.spawning_section(ui, world, content, cursor_tile)
            });
            self.window(ctx, 2, "Loot", Self::win_loot_get, |s, ui| {
                s.loot_section(ui, world, content, cursor_tile)
            });
            self.window(ctx, 3, "Movement / pathing", Self::win_movement_get, |_s, ui| {
                Self::movement_body(ui, world)
            });
            self.window(ctx, 4, "Level / viz", Self::win_level_get, |s, ui| {
                s.level_body(ui, world)
            });
            self.window(ctx, 5, "Controller", Self::win_controller_get, |_s, ui| {
                controller_body(ui, pad_diag)
            });
            self.window(ctx, 6, "World actions", Self::win_actions_get, |_s, ui| {
                Self::actions_body(ui, world)
            });
            self.window(ctx, 7, "Tunables file", Self::win_file_get, |s, ui| {
                s.file_body(ui, world)
            });

            // Read after everything is built so it reflects this frame's
            // interaction (used to suppress firing through a panel).
            wants_pointer = ctx.wants_pointer_input();
        });
        wants_pointer
    }

    // Field accessors so `window()` can address an open-bool generically without
    // a borrow-checker fight (it copies via the getter, writes back via the ptr).
    fn win_combat_get(&mut self) -> &mut bool { &mut self.win_combat }
    fn win_spawning_get(&mut self) -> &mut bool { &mut self.win_spawning }
    fn win_loot_get(&mut self) -> &mut bool { &mut self.win_loot }
    fn win_movement_get(&mut self) -> &mut bool { &mut self.win_movement }
    fn win_level_get(&mut self) -> &mut bool { &mut self.win_level }
    fn win_controller_get(&mut self) -> &mut bool { &mut self.win_controller }
    fn win_actions_get(&mut self) -> &mut bool { &mut self.win_actions }
    fn win_file_get(&mut self) -> &mut bool { &mut self.win_file }

    /// Render one collapsible/closable tool window. `open` selects the backing
    /// bool; `body` fills it. The open bool is copied to a local for egui's
    /// `.open()` (its close [x] toggles the local), then written back — so the
    /// body closure is free to borrow `self` without aliasing the bool.
    fn window(
        &mut self,
        ctx: &egui::Context,
        slot: usize,
        title: &str,
        open: fn(&mut Self) -> &mut bool,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        let mut is_open = *open(self);
        if is_open {
            // Cascade the default position out to the *left* of the top-right
            // launcher so a freshly-opened tool lands near it (title bar
            // reachable, fully on-screen) instead of egui's centre-stack. Only
            // applies until the user drags it — egui then remembers the moved
            // position by id. Width 300; launcher occupies ~220 at the right edge.
            let sr = ctx.screen_rect();
            let pos = [
                sr.right() - 540.0 - slot as f32 * 24.0,
                sr.top() + 8.0 + slot as f32 * 28.0,
            ];
            egui::Window::new(title)
                .open(&mut is_open)
                .default_pos(pos)
                .default_width(300.0)
                .resizable(true)
                .show(ctx, |ui| {
                    // `drag_to_scroll(false)`: a click with slight cursor drift
                    // (common on a trackpad) would otherwise scroll-drag and eat
                    // the button click.
                    egui::ScrollArea::vertical()
                        .drag_to_scroll(false)
                        .show(ui, |ui| body(self, ui));
                });
        }
        *open(self) = is_open;
    }

    /// Paint the windows. Separate from [`run`] so the caller controls draw
    /// ordering (egui goes on top, after the 2D HUD).
    pub fn draw(&self) {
        if self.visible {
            egui_macroquad::draw();
        }
    }

    /// The launcher window body: god mode, a grid of tool-window toggles, and the
    /// live stats footer.
    fn launcher_body(&mut self, ui: &mut egui::Ui, world: &mut World) {
        ui.add_space(2.0);
        ui.checkbox(&mut world.tunables.god_mode, "god mode  ·  no contact damage");
        ui.add_space(4.0);
        ui.label(egui::RichText::new("WINDOWS").weak().size(10.5));
        egui::Grid::new("window_toggles")
            .num_columns(2)
            .spacing([12.0, 3.0])
            .show(ui, |ui| {
                ui.checkbox(&mut self.win_combat, "Combat");
                ui.checkbox(&mut self.win_spawning, "Spawning");
                ui.end_row();
                ui.checkbox(&mut self.win_loot, "Loot");
                ui.checkbox(&mut self.win_movement, "Movement");
                ui.end_row();
                ui.checkbox(&mut self.win_level, "Level / viz");
                ui.checkbox(&mut self.win_controller, "Controller");
                ui.end_row();
                ui.checkbox(&mut self.win_actions, "Actions");
                ui.checkbox(&mut self.win_file, "Tunables file");
                ui.end_row();
            });
        Self::stats_footer(ui, world);
    }

    /// Flat: weapon picker + combat sliders + live TTK as one block. Equipping a
    /// weapon loads its stats into these very sliders, and the readout reflects
    /// them against the manual-spawn archetype — equip, tune, watch TTK move.
    fn loadout_section(&mut self, ui: &mut egui::Ui, world: &mut World, content: &Content) {
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
            // Replaces the active rack slot (so tuning doesn't pile up dupes) and
            // loads the weapon's stats into these very sliders (the fire path's
            // read surface), so equip → tune → watch TTK move still works.
            world.debug_equip_base(&base.id, content);
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

        // Archetype fire-pattern knobs — shown only for the profile the equipped
        // weapon actually uses (seeded from the weapon on equip, live here). The
        // fire path reads these tunables, so a drag retunes the next shot.
        match world.equipped().map(|e| e.profile) {
            Some(FireProfile::Spread { .. }) => {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("shotgun spread").weak().size(11.5));
                let t = &mut world.tunables;
                ui.add(egui::Slider::new(&mut t.spread_pellets, 1..=16).text("pellets"));
                ui.add(egui::Slider::new(&mut t.spread_angle, 0.0..=1.2).text("half-angle rad"));
            }
            Some(FireProfile::Explosive { .. }) => {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("rocket blast").weak().size(11.5));
                let t = &mut world.tunables;
                ui.add(egui::Slider::new(&mut t.blast_radius, 0.2..=6.0).text("blast radius"));
                ui.add(egui::Slider::new(&mut t.blast_speed_factor, 0.1..=1.5).text("rocket speed ×"));
            }
            _ => {}
        }

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
        let t = &mut world.tunables;
        ui.checkbox(&mut t.auto_spawn, "auto-spawn waves");
        ui.add(egui::Slider::new(&mut t.spawn_interval, 0.2..=15.0).text("interval s"));
        ui.add(egui::Slider::new(&mut t.spawn_batch, 1..=20).text("batch"));
        ui.add(egui::Slider::new(&mut t.max_enemies, 1..=200).text("max enemies"));
        ui.add(egui::Slider::new(&mut t.spawn_min_distance, 1..=40).text("min distance"));
        ui.add(egui::Slider::new(&mut t.drop_chance, 0.0..=1.0).text("drop chance"));

        // Per-archetype wave composition. Each wave draws its batch from these
        // relative weights (0 = never spawn that type). All-zero ⇒ uniform, so
        // the buttons make it easy to get back to shipping behaviour.
        subhead(ui, "wave composition (weights)");
        let n = content.enemies.len().min(h2b_game::MAX_SPAWN_ARCHETYPES);
        for i in 0..n {
            let name = content.enemies[i].id.as_str();
            ui.add(egui::Slider::new(&mut world.tunables.spawn_weights[i], 0..=10).text(name));
        }
        ui.horizontal(|ui| {
            if ui.button("uniform (clear)").clicked() {
                world.tunables.spawn_weights = [0; h2b_game::MAX_SPAWN_ARCHETYPES];
            }
            if ui.button("all ×1").clicked() {
                world.tunables.spawn_weights = [0; h2b_game::MAX_SPAWN_ARCHETYPES];
                for w in world.tunables.spawn_weights.iter_mut().take(n) {
                    *w = 1;
                }
            }
        });

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

    /// Flat: roll and drop a loot item on demand — pick a base (or random),
    /// rarity, and ilvl, then drop it on the player (instant pickup, for filling
    /// the rack/bag) or at the cursor (lands on the ground to walk over).
    fn loot_section(
        &mut self,
        ui: &mut egui::Ui,
        world: &mut World,
        content: &Content,
        cursor_tile: Option<(f32, f32)>,
    ) {
        let base_label = if self.drop_base == 0 {
            "(random)"
        } else {
            content
                .bases
                .get(self.drop_base - 1)
                .map(|b| b.name.as_str())
                .unwrap_or("—")
        };
        egui::ComboBox::from_label("base")
            .selected_text(base_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.drop_base, 0, "(random)");
                for (i, b) in content.bases.iter().enumerate() {
                    ui.selectable_value(&mut self.drop_base, i + 1, b.name.as_str());
                }
            });
        egui::ComboBox::from_label("rarity")
            .selected_text(format!("{:?}", self.drop_rarity))
            .show_ui(ui, |ui| {
                for r in [
                    Rarity::Basic,
                    Rarity::Common,
                    Rarity::Rare,
                    Rarity::Epic,
                    Rarity::Legendary,
                ] {
                    ui.selectable_value(&mut self.drop_rarity, r, format!("{r:?}"));
                }
            });
        ui.add(egui::Slider::new(&mut self.drop_ilvl, 1..=60).text("ilvl"));
        ui.checkbox(&mut self.drop_at_cursor, "at cursor (else on the player)");

        ui.horizontal(|ui| {
            if ui.button("drop item").clicked() {
                let base = if self.drop_base == 0 {
                    None
                } else {
                    content.bases.get(self.drop_base - 1).map(|b| b.id.as_str())
                };
                let (x, y) = if self.drop_at_cursor {
                    cursor_tile.unwrap_or((world.player.x, world.player.y))
                } else {
                    (world.player.x, world.player.y)
                };
                world.debug_drop(x, y, base, self.drop_rarity, self.drop_ilvl, content);
            }
            if ui.button("clear drops").clicked() {
                world.debug_clear_drops();
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
        ui.add(egui::Slider::new(&mut t.player_accel, 5.0..=200.0).text("player accel"));
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

/// A minor divider label inside a section (e.g. "manual" within Spawning).
fn subhead(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text.to_uppercase()).weak().size(10.5));
    ui.add_space(1.0);
}

/// Live controller diagnostics — surfaces whether gilrs sees a pad, its SDL
/// mapping (the usual "connected but dead" culprit when missing), and live
/// stick/trigger/button state so you can confirm inputs are reaching the game.
fn controller_body(ui: &mut egui::Ui, diag: &crate::PadDiag) {
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
