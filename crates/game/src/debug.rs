//! Debug / tuning overlay — compiled only under the `debug` feature.
//!
//! An immediate-mode egui panel bound directly to `World::tunables` plus the
//! `World::debug_*` spawn helpers. Every widget here edits live game state; the
//! headless `bb_game` library knows nothing about it. Toggle with **F1**.
//!
//! Bevy port: the panel body (windows / grids / sliders bound to
//! `&mut world.tunables`) is unchanged — only the egui *host* moved from
//! `egui-macroquad` to `bevy_egui`. `DebugUi` is a `Resource`; `debug_panel` is
//! a system in the `EguiPrimaryContextPass` schedule. The old flow-field and
//! entity-stat viz (was macroquad immediate draws in `render.rs`) are
//! re-authored here as Bevy gizmos + an egui overlay.
//!
//! Run it with:
//! ```text
//! cargo run -p bigbox-game --features debug
//! ```

use crate::{Aim, BlockFire, FollowCam, GameContent, Sim};
use bb_core::{HitResult, Rarity, Weapon, dps_against, time_to_kill};
use bb_game::{Content, FireProfile, Tunables, World};
use bb_procgen::{ArenaParams, Map, MapParams, generate_arena, generate_bsp};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

/// Where tunables export/import. Lives in the process working directory.
const TUNABLES_PATH: &str = "bigbox-tunables.ron";

/// Set when a map-swap button rebuilds the world onto a new map, so
/// `scene::reload_map_geometry` respawns the static geometry next frame.
#[derive(Resource, Default)]
pub struct MapDirty(pub bool);

/// A connected gamepad's live state, distilled from the `bevy_gilrs` `Gamepad`
/// component for the controller panel. (bevy_gilrs doesn't expose the SDL GUID /
/// mapping name / raw pre-mapping axis codes the old raw-`gilrs` diag showed, so
/// those are gone — vendor/product id is the closest identity handle left.)
pub struct PadView {
    name: String,
    vendor: Option<u16>,
    product: Option<u16>,
    left_stick: (f32, f32),
    right_stick: (f32, f32),
    right_trigger: f32,
    buttons_down: Vec<&'static str>,
}

#[derive(Resource)]
pub struct DebugUi {
    visible: bool,
    archetype: usize,
    spawn_count: i32,
    spawn_distance: u32,
    spawn_at_cursor: bool,
    weapon_base: usize,
    drop_base: usize,
    drop_rarity: Rarity,
    drop_ilvl: u32,
    drop_at_cursor: bool,
    arena_pillars: bool,
    show_flow: bool,
    show_entity_stats: bool,
    win_combat: bool,
    win_spawning: bool,
    win_loot: bool,
    win_movement: bool,
    win_level: bool,
    win_controller: bool,
    win_actions: bool,
    win_file: bool,
    status: String,
    /// Set by a map-swap button; drained by `debug_panel` into `MapDirty`.
    map_reloaded: bool,
}

impl Default for DebugUi {
    fn default() -> Self {
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
            win_combat: false,
            win_spawning: false,
            win_loot: false,
            win_movement: false,
            win_level: false,
            win_controller: false,
            win_actions: false,
            win_file: false,
            status: String::new(),
            map_reloaded: false,
        }
    }
}

/// The tuning panel — a `bevy_egui` system in the `EguiPrimaryContextPass`
/// schedule. F1 toggles it; while it wants the pointer it sets `BlockFire` so
/// clicks on the panel don't shoot/switch through to the game.
pub fn debug_panel(
    mut contexts: EguiContexts,
    mut ui_state: ResMut<DebugUi>,
    mut sim: ResMut<Sim>,
    content: Res<GameContent>,
    aim: Res<Aim>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut block: ResMut<BlockFire>,
    mut map_dirty: ResMut<MapDirty>,
    pads: Query<(&Name, &Gamepad)>,
) -> Result {
    if keys.just_pressed(KeyCode::F1) {
        ui_state.visible = !ui_state.visible;
    }
    if !ui_state.visible {
        block.0 = false;
        return Ok(());
    }

    let ctx = contexts.ctx_mut()?;
    // Forgive cursor jitter between press and release (trackpad / high-DPI) so
    // egui doesn't reclassify a click as a drag and drop it.
    ctx.options_mut(|o| o.input_options.max_click_dist = 14.0);

    let content = &content.0;
    let cursor_tile = aim.hit.map(|h| (h.x, h.z));
    let fps = 1.0 / time.delta_secs().max(1e-4);
    let pad_views: Vec<PadView> = pads.iter().map(|(n, g)| PadView::from_gamepad(n, g)).collect();

    let ui_state = &mut *ui_state;
    let world = &mut sim.0;

    // Launcher: compact always-on window with god mode, tool toggles, footer.
    egui::Window::new("debug (F1)")
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
        .default_width(220.0)
        .show(ctx, |ui| ui_state.launcher_body(ui, world, fps));

    ui_state.window(ctx, 0, "Weapon & Combat", DebugUi::win_combat_get, |s, ui| {
        s.loadout_section(ui, world, content)
    });
    ui_state.window(ctx, 1, "Spawning", DebugUi::win_spawning_get, |s, ui| {
        s.spawning_section(ui, world, content, cursor_tile)
    });
    ui_state.window(ctx, 2, "Loot", DebugUi::win_loot_get, |s, ui| {
        s.loot_section(ui, world, content, cursor_tile)
    });
    ui_state.window(ctx, 3, "Movement / pathing", DebugUi::win_movement_get, |_s, ui| {
        DebugUi::movement_body(ui, world)
    });
    ui_state.window(ctx, 4, "Level / viz", DebugUi::win_level_get, |s, ui| {
        s.level_body(ui, world)
    });
    ui_state.window(ctx, 5, "Controller", DebugUi::win_controller_get, |_s, ui| {
        controller_body(ui, &pad_views)
    });
    ui_state.window(ctx, 6, "World actions", DebugUi::win_actions_get, |_s, ui| {
        DebugUi::actions_body(ui, world)
    });
    ui_state.window(ctx, 7, "Tunables file", DebugUi::win_file_get, |s, ui| {
        s.file_body(ui, world)
    });

    block.0 = ctx.egui_wants_pointer_input();

    // A map-swap button rebuilt the world; ask the renderer to respawn geometry.
    if ui_state.map_reloaded {
        ui_state.map_reloaded = false;
        map_dirty.0 = true;
    }
    Ok(())
}

impl DebugUi {
    fn win_combat_get(&mut self) -> &mut bool { &mut self.win_combat }
    fn win_spawning_get(&mut self) -> &mut bool { &mut self.win_spawning }
    fn win_loot_get(&mut self) -> &mut bool { &mut self.win_loot }
    fn win_movement_get(&mut self) -> &mut bool { &mut self.win_movement }
    fn win_level_get(&mut self) -> &mut bool { &mut self.win_level }
    fn win_controller_get(&mut self) -> &mut bool { &mut self.win_controller }
    fn win_actions_get(&mut self) -> &mut bool { &mut self.win_actions }
    fn win_file_get(&mut self) -> &mut bool { &mut self.win_file }

    /// Render one closable tool window. `open` selects the backing bool; `body`
    /// fills it. The bool is copied to a local for egui's `.open()` (its close
    /// [x] toggles the local), then written back so `body` can borrow `self`.
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
            let sr = ctx.content_rect();
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
                    egui::ScrollArea::vertical().show(ui, |ui| body(self, ui));
                });
        }
        *open(self) = is_open;
    }

    fn launcher_body(&mut self, ui: &mut egui::Ui, world: &mut World, fps: f32) {
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
        Self::stats_footer(ui, world, fps);
    }

    fn loadout_section(&mut self, ui: &mut egui::Ui, world: &mut World, content: &Content) {
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

        subhead(ui, "wave composition (weights)");
        let n = content.enemies.len().min(bb_game::MAX_SPAWN_ARCHETYPES);
        for i in 0..n {
            let name = content.enemies[i].id.as_str();
            ui.add(egui::Slider::new(&mut world.tunables.spawn_weights[i], 0..=10).text(name));
        }
        ui.horizontal(|ui| {
            if ui.button("uniform (clear)").clicked() {
                world.tunables.spawn_weights = [0; bb_game::MAX_SPAWN_ARCHETYPES];
            }
            if ui.button("all ×1").clicked() {
                world.tunables.spawn_weights = [0; bb_game::MAX_SPAWN_ARCHETYPES];
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

    fn level_body(&mut self, ui: &mut egui::Ui, world: &mut World) {
        ui.checkbox(&mut self.arena_pillars, "arena pillars (pathing obstacles)");
        ui.horizontal(|ui| {
            if ui.button("load arena").clicked() {
                let map = generate_arena(&ArenaParams {
                    pillars: self.arena_pillars,
                    ..Default::default()
                });
                reload_world(world, map);
                world.tunables.auto_spawn = false;
                self.map_reloaded = true;
            }
            if ui.button("load BSP").clicked() {
                let map = generate_bsp(&MapParams {
                    seed: 42,
                    ..Default::default()
                });
                reload_world(world, map);
                self.map_reloaded = true;
            }
        });
        ui.checkbox(&mut self.show_flow, "show flow field (enemy pathing)");
        ui.checkbox(&mut self.show_entity_stats, "show entity stats (floating)");
    }

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

    fn stats_footer(ui: &mut egui::Ui, world: &World, fps: f32) {
        ui.add_space(6.0);
        ui.separator();
        ui.label(format!("fps {fps:.0}"));
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
            Some(HitResult::Hit { damage_dealt, was_crit }) => {
                format!("last hit {:.1}{}", damage_dealt, if *was_crit { " CRIT" } else { "" })
            }
            Some(HitResult::Dodged) => "last hit DODGED".to_string(),
            None => "last hit —".to_string(),
        };
        ui.label(last);
    }
}

/// Flow-field pathing viz — cyan arrows down the field toward the player, a
/// yellow pad on the goal tile. Bevy `Gizmos` port of the old `draw_flow_field`;
/// reads the same `steer_from` (+ discrete-saddle fallback) enemies follow.
pub fn debug_flow_gizmos(ui_state: Res<DebugUi>, sim: Res<Sim>, mut gizmos: Gizmos) {
    use bb_procgen::{Tile, UNREACHABLE};
    if !(ui_state.visible && ui_state.show_flow) {
        return;
    }
    let world = &sim.0;
    let flow = world.flow();
    let map = &world.map;
    let (px, py) = (world.player.x, world.player.y);
    const R2: f32 = 30.0 * 30.0;
    for ty in 0..map.height {
        for tx in 0..map.width {
            if !matches!(map.tile_at(tx, ty), Tile::Floor) {
                continue;
            }
            let (cx, cz) = (tx as f32 + 0.5, ty as f32 + 0.5);
            if (cx - px).powi(2) + (cz - py).powi(2) > R2 {
                continue;
            }
            if flow.distance_at(tx, ty) == UNREACHABLE {
                continue;
            }
            match flow.steer_from(cx, cz).or_else(|| flow.next_step_dir(cx, cz)) {
                Some((dx, dz)) => {
                    let from = Vec3::new(cx, 0.06, cz);
                    let to = Vec3::new(cx + dx * 0.4, 0.06, cz + dz * 0.4);
                    gizmos.line(from, to, Color::srgba(0.30, 0.80, 1.00, 0.7));
                }
                None => {
                    gizmos.cube(
                        Transform::from_xyz(cx, 0.06, cz).with_scale(Vec3::new(0.18, 0.02, 0.18)),
                        Color::srgba(1.00, 1.00, 0.40, 0.85),
                    );
                }
            }
        }
    }
}

/// Floating Enemy/Combatant stat blocks above the nearest few enemies + a PLAYER
/// block — an egui-overlay port of the old `draw_entity_stats`. World anchor →
/// screen via `Camera::world_to_viewport`; labels are drawn as fixed-pos egui
/// areas. Nearest `MAX_LABELS` enemies only; lowest id drawn last (on top).
pub fn debug_entity_stats(
    mut contexts: EguiContexts,
    ui_state: Res<DebugUi>,
    sim: Res<Sim>,
    content: Res<GameContent>,
    cam: Query<(&Camera, &GlobalTransform), With<FollowCam>>,
) -> Result {
    use bb_procgen::UNREACHABLE;
    const MAX_LABELS: usize = 5;
    if !(ui_state.visible && ui_state.show_entity_stats) {
        return Ok(());
    }
    let Ok((camera, cam_xf)) = cam.single() else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let world = &sim.0;
    let (px, py) = (world.player.x, world.player.y);

    let mut ranked: Vec<(f32, usize)> = world
        .enemies
        .iter()
        .enumerate()
        .map(|(i, e)| ((e.x - px).powi(2) + (e.y - py).powi(2), i))
        .collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    ranked.truncate(MAX_LABELS);
    // Lowest id last → drawn on top when boxes overlap.
    ranked.sort_by(|a, b| world.enemies[b.1].id.cmp(&world.enemies[a.1].id));

    let label = |ctx: &egui::Context, id: &str, world_pos: Vec3, lines: &[String], accent: egui::Color32| {
        if let Ok(screen) = camera.world_to_viewport(cam_xf, world_pos) {
            egui::Area::new(egui::Id::new(("dbg_stat", id)))
                .fixed_pos(egui::pos2(screen.x, screen.y))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::default()
                        .fill(egui::Color32::from_black_alpha(160))
                        .inner_margin(4.0)
                        .show(ui, |ui| {
                            for l in lines {
                                ui.label(egui::RichText::new(l).size(12.0).color(accent));
                            }
                        });
                });
        }
    };

    let accent = egui::Color32::from_rgb(0xf7, 0xd9, 0x8c);
    for &(d2, i) in &ranked {
        let e = &world.enemies[i];
        let id = content
            .0
            .enemies
            .get(e.archetype)
            .map(|a| a.id.as_str())
            .unwrap_or("?");
        let c = &e.combatant;
        let ilvl = content.0.enemies.get(e.archetype).map(|a| a.ilvl).unwrap_or(0);
        let flow_d = world.flow().distance_at(e.x as u32, e.y as u32);
        let flow_s = if flow_d == UNREACHABLE { "∞".into() } else { flow_d.to_string() };
        let state = if e.awake { "" } else { "  [idle]" };
        let lines = [
            format!("{id} #{}  il{ilvl}{state}", e.id),
            format!("hp {:.0}/{:.0}", c.current_life, c.max_life),
            format!("arm {:.0}  eva {:.0}", c.armor, c.evasion),
            format!("spd {:.1}  d {:.1}  flow {flow_s}", e.speed, d2.sqrt()),
        ];
        label(ctx, &format!("e{}", e.id), Vec3::new(e.x, 1.9, e.y), &lines, accent);
    }

    let p = &world.player;
    let lines = [
        "PLAYER".to_string(),
        format!("hp {:.0}/{:.0}", p.current_life, p.max_life),
        format!("pos {:.1}, {:.1}  cd {:.2}", p.x, p.y, world.player_fire_cooldown),
        format!("enemies {}  proj {}", world.enemies.len(), world.projectiles.len()),
    ];
    label(
        ctx,
        "player",
        Vec3::new(px, 1.9, py),
        &lines,
        egui::Color32::from_rgb(0x8c, 0xf7, 0x9e),
    );
    Ok(())
}

impl PadView {
    fn from_gamepad(name: &Name, gp: &Gamepad) -> Self {
        const BUTTONS: &[(&str, GamepadButton)] = &[
            ("A", GamepadButton::South),
            ("B", GamepadButton::East),
            ("X", GamepadButton::West),
            ("Y", GamepadButton::North),
            ("LB", GamepadButton::LeftTrigger),
            ("RB", GamepadButton::RightTrigger),
            ("LT", GamepadButton::LeftTrigger2),
            ("RT", GamepadButton::RightTrigger2),
            ("Start", GamepadButton::Start),
            ("Select", GamepadButton::Select),
            ("Up", GamepadButton::DPadUp),
            ("Down", GamepadButton::DPadDown),
            ("Left", GamepadButton::DPadLeft),
            ("Right", GamepadButton::DPadRight),
        ];
        let ls = gp.left_stick();
        let rs = gp.right_stick();
        PadView {
            name: name.as_str().to_string(),
            vendor: gp.vendor_id(),
            product: gp.product_id(),
            left_stick: (ls.x, ls.y),
            right_stick: (rs.x, rs.y),
            right_trigger: gp.get(GamepadButton::RightTrigger2).unwrap_or(0.0),
            buttons_down: BUTTONS.iter().filter(|(_, b)| gp.pressed(*b)).map(|(n, _)| *n).collect(),
        }
    }
}

/// Live controller diagnostics from `bevy_gilrs`. Confirms a pad is seen and its
/// stick/trigger/button state is reaching the game.
fn controller_body(ui: &mut egui::Ui, pads: &[PadView]) {
    if pads.is_empty() {
        ui.colored_label(egui::Color32::YELLOW, "no gamepad detected");
        ui.label("(on macOS, Xbox controllers over USB use a proprietary");
        ui.label(" protocol IOKit/HID can't read — try Bluetooth pairing)");
        return;
    }
    for (i, p) in pads.iter().enumerate() {
        ui.colored_label(egui::Color32::LIGHT_GREEN, format!("[{i}] {}", p.name));
        if let (Some(v), Some(pr)) = (p.vendor, p.product) {
            ui.label(format!("  vendor {v:#06x}  product {pr:#06x}"));
        }
        ui.label(format!("  L-stick: ({:+.2}, {:+.2})", p.left_stick.0, p.left_stick.1));
        ui.label(format!("  R-stick: ({:+.2}, {:+.2})", p.right_stick.0, p.right_stick.1));
        ui.label(format!("  RT: {:.2}", p.right_trigger));
        let btns = if p.buttons_down.is_empty() {
            "—".to_string()
        } else {
            p.buttons_down.join(", ")
        };
        ui.label(format!("  buttons: {btns}"));
    }
}

fn subhead(ui: &mut egui::Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text.to_uppercase()).weak().size(10.5));
    ui.add_space(1.0);
}

/// Swap the world onto a new map, preserving tunables (a fresh `World` would
/// reset them). Inventory / kills / XP reset — a fresh level, not a checkpoint.
fn reload_world(world: &mut World, map: Map) {
    let saved = world.tunables;
    *world = World::new(map);
    world.tunables = saved;
}

fn export(tunables: &Tunables) -> Result<String, String> {
    let ron = ron::ser::to_string_pretty(tunables, ron::ser::PrettyConfig::default())
        .map_err(|e| e.to_string())?;
    std::fs::write(TUNABLES_PATH, ron).map_err(|e| e.to_string())?;
    let abs = std::env::current_dir()
        .map(|d| d.join(TUNABLES_PATH).display().to_string())
        .unwrap_or_else(|_| TUNABLES_PATH.to_string());
    Ok(abs)
}

fn import() -> Result<Tunables, String> {
    let text = std::fs::read_to_string(TUNABLES_PATH).map_err(|e| e.to_string())?;
    ron::from_str(&text).map_err(|e| e.to_string())
}
