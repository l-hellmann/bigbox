//! Lightweight 2D UI, drawn with macroquad primitives (no egui — that's
//! debug/native-only and can't link on wasm). Just enough widgets for the
//! in-game menus: a panel frame, a clickable button, and an item slot with
//! hover/click hit-testing. Immediate-mode: each `draw_*` both renders and
//! reports what was clicked this frame; the caller applies the resulting action
//! so these stay read-only on the world.

use crate::render::rarity_color;
use h2b_game::{Content, World};
use macroquad::prelude::*;

// ---- theme ----
const SCREEN_DIM: Color = Color::new(0.0, 0.0, 0.0, 0.55);
// Fully opaque: at <1.0 the frozen 3D scene (the player/enemy cubes at screen
// center) bleeds through and reads as a stray dark box on the panel.
const PANEL_BG: Color = Color::new(0.06, 0.07, 0.09, 1.0);
const PANEL_BORDER: Color = Color::new(0.22, 0.26, 0.32, 1.0);
const SLOT_BG: Color = Color::new(0.11, 0.13, 0.16, 1.0);
const SLOT_HOVER: Color = Color::new(0.18, 0.22, 0.27, 1.0);
const TEXT: Color = Color::new(0.88, 0.90, 0.92, 1.0);
const TEXT_DIM: Color = Color::new(0.55, 0.58, 0.62, 1.0);
const ACTIVE_BORDER: Color = Color::new(0.55, 0.95, 0.60, 1.0);

/// Load the bundled UI font (Quantico, SIL OFL 1.1 — see
/// `assets/fonts/quantico/OFL.txt`). Embedded via `include_bytes!` so it needs
/// no runtime file read and works identically on native and wasm (matching the
/// content crate's `include_str!` approach).
pub fn load_font() -> Font {
    load_ttf_font_from_bytes(include_bytes!("../assets/fonts/quantico/Quantico-Regular.ttf"))
        .expect("bundled UI font should parse")
}

/// Draw `s` in the UI font. Thin wrapper over `draw_text_ex` so callers keep the
/// terse `(text, x, y, size, color)` shape they had with `draw_text`.
pub fn text(font: &Font, s: &str, x: f32, y: f32, size: f32, color: Color) {
    draw_text_ex(
        s,
        x,
        y,
        TextParams {
            font: Some(font),
            font_size: size as u16,
            color,
            ..Default::default()
        },
    );
}

/// Measure `s` in the UI font (for centering).
pub fn measure(font: &Font, s: &str, size: f32) -> TextDimensions {
    measure_text(s, Some(font), size as u16, 1.0)
}

/// An axis-aligned rectangle in screen pixels, with a mouse hit test.
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Rect { x, y, w, h }
    }

    pub fn contains(&self, (mx, my): (f32, f32)) -> bool {
        mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h
    }
}

/// A filled, bordered panel.
pub fn panel(r: Rect) {
    draw_rectangle(r.x, r.y, r.w, r.h, PANEL_BG);
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 2.0, PANEL_BORDER);
}

/// A clickable button: hover-lit fill, centered label. Returns whether it was
/// clicked this frame (`mouse` over it and `click` edge set).
pub fn button(font: &Font, r: Rect, label: &str, mouse: (f32, f32), click: bool) -> bool {
    let over = r.contains(mouse);
    draw_rectangle(r.x, r.y, r.w, r.h, if over { SLOT_HOVER } else { SLOT_BG });
    draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.5, PANEL_BORDER);
    let d = measure(font, label, 18.0);
    text(
        font,
        label,
        r.x + (r.w - d.width) * 0.5,
        r.y + (r.h + d.height) * 0.5,
        18.0,
        TEXT,
    );
    over && click
}

/// What the inventory screen reports the player did this frame.
pub enum InvAction {
    /// Close the inventory.
    Close,
    /// Switch the active weapon to rack slot `usize`.
    Switch(usize),
    /// Equip the inventory item at `usize` into the rack.
    Equip(usize),
}

/// Draw the inventory overlay and return any action the click triggered. Pure
/// read of the world — the caller mutates in response.
///
/// Layout: a centered modal — the equipped weapon rack (click to switch) on
/// top, then the collected-item grid below (click a weapon to equip; armor is
/// listed but inert for now).
pub fn draw_inventory(
    font: &Font,
    world: &World,
    content: &Content,
    mouse: (f32, f32),
    click: bool,
) -> Option<InvAction> {
    let (sw, sh) = (screen_width(), screen_height());
    draw_rectangle(0.0, 0.0, sw, sh, SCREEN_DIM);

    let pw = 700.0_f32.min(sw - 40.0);
    let ph = 500.0_f32.min(sh - 40.0);
    let px = (sw - pw) * 0.5;
    let py = (sh - ph) * 0.5;
    panel(Rect::new(px, py, pw, ph));

    text(font, "INVENTORY", px + 20.0, py + 34.0, 28.0, TEXT);

    let mut action = None;

    // Close button, top-right.
    if button(font, Rect::new(px + pw - 44.0, py + 14.0, 30.0, 30.0), "x", mouse, click) {
        action = Some(InvAction::Close);
    }

    // ---- equipped rack row ----
    text(font, "EQUIPPED", px + 20.0, py + 66.0, 16.0, TEXT_DIM);
    let slot_w = 156.0;
    let slot_h = 46.0;
    let gap = 10.0;
    for (i, w) in world.loadout().iter().enumerate() {
        let r = Rect::new(px + 20.0 + i as f32 * (slot_w + gap), py + 76.0, slot_w, slot_h);
        let over = r.contains(mouse);
        let active = i == world.active_slot();
        draw_rectangle(r.x, r.y, r.w, r.h, if over { SLOT_HOVER } else { SLOT_BG });
        let border = if active { ACTIVE_BORDER } else { rarity_color(w.item.rarity) };
        draw_rectangle_lines(r.x, r.y, r.w, r.h, if active { 2.5 } else { 1.5 }, border);
        text(font, &format!("{}: {}", i + 1, w.name), r.x + 8.0, r.y + 20.0, 16.0, TEXT);
        text(
            font,
            &format!("dmg {:.0}  rate {:.1}", w.weapon.damage_per_shot, w.weapon.fire_rate),
            r.x + 8.0,
            r.y + 38.0,
            13.0,
            TEXT_DIM,
        );
        if over && click {
            action = Some(InvAction::Switch(i));
        }
    }

    // ---- divider + collected-item grid ----
    // Well below the rack row (which ends ~py+122) so the "BAG" heading and
    // divider don't overlap the equipped slots.
    let grid_top = py + 178.0;
    draw_line(px + 16.0, grid_top - 16.0, px + pw - 16.0, grid_top - 16.0, 1.0, PANEL_BORDER);
    text(font, "BAG", px + 20.0, grid_top - 22.0, 16.0, TEXT_DIM);

    if world.inventory.is_empty() {
        text(
            font,
            "(empty — walk over drops to collect them)",
            px + 20.0,
            grid_top + 16.0,
            16.0,
            TEXT_DIM,
        );
    }

    let cols = 4;
    let cell_w = (pw - 40.0 - gap * (cols as f32 - 1.0)) / cols as f32;
    let cell_h = 56.0;
    let footer_y = py + ph - 18.0;
    for (i, item) in world.inventory.iter().enumerate() {
        let col = i % cols;
        let row = i / cols;
        let cx = px + 20.0 + col as f32 * (cell_w + gap);
        let cy = grid_top + row as f32 * (cell_h + gap);
        // Stop before overrunning the footer.
        if cy + cell_h > footer_y - 8.0 {
            break;
        }
        let r = Rect::new(cx, cy, cell_w, cell_h);
        let base = content.bases.iter().find(|b| b.id == item.base);
        let is_weapon = base.map(|b| b.slot == "weapon").unwrap_or(false);
        let name = base.map(|b| b.name.as_str()).unwrap_or(item.base.as_str());
        let over = r.contains(mouse);

        draw_rectangle(
            r.x,
            r.y,
            r.w,
            r.h,
            if over && is_weapon { SLOT_HOVER } else { SLOT_BG },
        );
        draw_rectangle_lines(r.x, r.y, r.w, r.h, 1.5, rarity_color(item.rarity));
        let name_color = if is_weapon { TEXT } else { TEXT_DIM };
        text(font, name, r.x + 8.0, r.y + 22.0, 17.0, name_color);
        let sub = if is_weapon { "click to equip" } else { "armor" };
        text(
            font,
            &format!("{:?} · {sub}", item.rarity),
            r.x + 8.0,
            r.y + 42.0,
            13.0,
            TEXT_DIM,
        );

        if over && click && is_weapon {
            action = Some(InvAction::Equip(i));
        }
    }

    text(
        font,
        "click a weapon to equip  ·  I / Tab / Esc to close",
        px + 20.0,
        footer_y,
        14.0,
        TEXT_DIM,
    );

    action
}
