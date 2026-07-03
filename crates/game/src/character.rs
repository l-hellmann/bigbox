//! GLTF character rendering + animation — the payoff of the Bevy move
//! (CLAUDE.md "deferred epic"). Replaces the hand-built stacked-cube box-people
//! with Kenney's CC0 blocky characters (`assets/characters/kennys/*.glb`),
//! loaded through the `AssetServer` (dev hot-reload) and animated by their own
//! GLTF node-animation clips via Bevy's animation graph.
//!
//! The characters are *node-animated*, not skinned: each glb is six boxy body
//! parts (legs / torso / arms / head) whose transforms the clips drive. That
//! maps cleanly onto Bevy's `AnimationPlayer` — the loader attaches one to the
//! animation root under each spawned `SceneRoot`, and we wire it to a per-letter
//! `AnimationGraph` the first frame it appears.
//!
//! The reconcile still lives in `scene.rs` (id-keyed pooling, Transform follow);
//! this module owns the *visual*: what to spawn ([`spawn_character`]), how it's
//! mapped from an enemy archetype ([`enemy_visual`]), and the idle/walk/shoot
//! animation state ([`CharModel`] + the attach/drive systems).

use crate::Sim;
use bevy::gltf::GltfAssetLabel;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};
use std::collections::HashMap;
use std::time::Duration;

/// Native Kenney model height → world-tile multiplier. The per-archetype scale
/// in [`enemy_visual`] / [`PLAYER_SCALE`] is applied on top of this. Tuned so a
/// scale-`1.0` character stands ~1 tile tall.
const CHAR_BASE_SCALE: f32 = 0.5;
/// Radians added to a character's yaw. Kenney characters face +Z at rest, which
/// matches our gameplay heading (front = +Z), so no rotation offset is needed.
const MODEL_FACING_OFFSET: f32 = 0.0;
/// Ground speed (tiles/sec) above which a character plays its walk cycle instead
/// of idling.
const WALK_SPEED_THRESHOLD: f32 = 0.35;
/// Ground speed (tiles/sec) above which a character breaks into a run (sprint
/// clip) — fast archetypes and a sprinting player.
const SPRINT_SPEED_THRESHOLD: f32 = 3.2;
/// Distance (tiles) at which an awake enemy plays its melee-attack clip instead
/// of walking — matched to roughly a body-length so it swings on contact.
pub const ATTACK_RANGE: f32 = 0.9;
/// How long the player holds the shoot pose after a shot (seconds); each shot
/// re-arms it, so continuous fire stays in the shoot animation.
const SHOOT_HOLD: f32 = 0.28;

/// The player's character letter + world-scale. Distinct green-ish ranger so the
/// player reads apart from the swarm; retune freely (data-only, CC0 placeholder).
pub const PLAYER_CHAR: char = 'a';
pub const PLAYER_SCALE: f32 = 1.05;

/// The animation clips we load per character, in a fixed order. The values in
/// [`CLIP_GLTF_INDEX`] are the clip indices *within each glb* (same animation
/// roster across all 18 characters — see the glb inspection in the epic).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AnimClip {
    Idle = 0,
    Walk = 1,
    Sprint = 2,
    HoldBoth = 3,
    Shoot = 4,
    Attack = 5,
    /// Loaded into the graph and reserved for a death animation once the sim
    /// keeps a dying corpse around; no trigger trips it yet.
    #[allow(dead_code)]
    Die = 6,
}

/// GLTF animation index for each [`AnimClip`], in enum order. From the Kenney
/// roster: 1=idle, 2=walk, 3=sprint, 12=holding-both, 15=holding-both-shoot,
/// 16=attack-melee-right, 6=die.
const CLIP_GLTF_INDEX: [usize; 7] = [1, 2, 3, 12, 15, 16, 6];

/// A loaded character: its scene (a GLTF `WorldAsset`) + a ready animation graph
/// with one node per [`AnimClip`] (indices parallel to the enum).
struct LoadedChar {
    scene: Handle<WorldAsset>,
    graph: Handle<AnimationGraph>,
    nodes: Vec<AnimationNodeIndex>,
}

/// Per-letter loaded character assets, built once at startup. Only the letters
/// actually mapped to the player + enemy roster are loaded (not all 18).
#[derive(Resource, Default)]
pub struct CharAssets(HashMap<char, LoadedChar>);

/// Render-side player shoot latch: tracks the fire-cooldown rising edge to drive
/// the shoot animation without any sim change (see [`crate::scene::sync_player`]).
#[derive(Resource, Default)]
pub struct PlayerAnim {
    pub prev_cooldown: f32,
    pub shoot_ttl: f32,
}

/// Marks a spawned character's `WorldAssetRoot` and carries its desired
/// animation state. The reconcile writes `state` each frame;
/// [`drive_char_animation`] applies transitions when it changes. `player` is the
/// descendant `AnimationPlayer` entity, resolved once by
/// [`attach_char_animation`].
#[derive(Component)]
pub struct CharModel {
    pub letter: char,
    pub state: AnimClip,
    applied: Option<AnimClip>,
    player: Option<Entity>,
}

impl CharModel {
    pub fn new(letter: char, state: AnimClip) -> Self {
        Self {
            letter,
            state,
            applied: None,
            player: None,
        }
    }
}

/// (character letter, world-scale) per enemy archetype id. Placeholder CC0
/// Kenney characters chosen by rough vibe (tank = bulky, boss = big); retune
/// freely — it's pure data. Unknown ids fall back to the basic zombie look.
pub fn enemy_visual(id: &str) -> (char, f32) {
    match id {
        "swarm_rusher" => ('n', 0.8),
        "fast_zombie" => ('h', 0.9),
        "spitter" => ('k', 1.0),
        "basic_zombie" => ('c', 1.0),
        "fat_zombie" => ('m', 1.35),
        "patient_zero" => ('r', 1.9),
        _ => ('c', 1.0),
    }
}

/// Approximate world-space height of a character at `scale` — where a health bar
/// anchors above it. (`enemy_visual`'s scale × the native head height × base.)
pub fn char_height(scale: f32) -> f32 {
    // Native Kenney characters are ~2.0 units tall head-to-toe.
    2.0 * CHAR_BASE_SCALE * scale
}

/// Every character letter the game can spawn (player + enemy roster), so startup
/// loads exactly those glbs.
fn used_letters() -> HashSet<char> {
    let mut s = HashSet::default();
    s.insert(PLAYER_CHAR);
    for id in [
        "swarm_rusher",
        "fast_zombie",
        "spitter",
        "basic_zombie",
        "fat_zombie",
        "patient_zero",
    ] {
        s.insert(enemy_visual(id).0);
    }
    s
}

/// Load the glb scene + animation clips for every used character letter and
/// build a ready `AnimationGraph` for each. Runs once at startup; the loads are
/// async (handles resolve later), which the attach system waits on.
pub fn setup_characters(
    asset_server: Res<AssetServer>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut store: ResMut<CharAssets>,
) {
    for letter in used_letters() {
        let path = format!("characters/kennys/character-{letter}.glb");
        let scene = asset_server.load(GltfAssetLabel::Scene(0).from_asset(path.clone()));
        let clips: Vec<Handle<AnimationClip>> = CLIP_GLTF_INDEX
            .iter()
            .map(|&i| asset_server.load(GltfAssetLabel::Animation(i).from_asset(path.clone())))
            .collect();
        let (graph, nodes) = AnimationGraph::from_clips(clips);
        let graph = graphs.add(graph);
        store.0.insert(letter, LoadedChar { scene, graph, nodes });
    }
}

/// Spawn a character `SceneRoot` for `letter` at ground position `pos` facing
/// `yaw`, scaled by the archetype `scale`. Returns the root entity so the caller
/// tags it (`EnemyView` / `PlayerView`). Sets up the [`CharModel`] in `state`;
/// the animation wires itself up once the scene's `AnimationPlayer` appears.
pub fn spawn_character(
    commands: &mut Commands,
    assets: &CharAssets,
    letter: char,
    scale: f32,
    pos: Vec3,
    yaw: f32,
    state: AnimClip,
) -> Entity {
    let scene = assets
        .0
        .get(&letter)
        .map(|c| c.scene.clone())
        .unwrap_or_default();
    commands
        .spawn((
            WorldAssetRoot(scene),
            char_transform(pos, yaw, scale),
            CharModel::new(letter, state),
        ))
        .id()
}

/// The world transform for a character: ground position, heading yaw (+ model
/// facing offset), and uniform archetype scale.
pub fn char_transform(pos: Vec3, yaw: f32, scale: f32) -> Transform {
    Transform::from_translation(pos)
        .with_rotation(Quat::from_rotation_y(yaw + MODEL_FACING_OFFSET))
        .with_scale(Vec3::splat(CHAR_BASE_SCALE * scale))
}

/// Pick the idle/walk animation for a character moving at `speed` (tiles/sec).
/// `armed` characters (the player) idle in the gun-holding pose so the weapon
/// shows; enemies use the plain idle.
pub fn locomotion(speed: f32, armed: bool) -> AnimClip {
    if speed > SPRINT_SPEED_THRESHOLD {
        AnimClip::Sprint
    } else if speed > WALK_SPEED_THRESHOLD {
        AnimClip::Walk
    } else if armed {
        AnimClip::HoldBoth
    } else {
        AnimClip::Idle
    }
}

/// Wire each freshly-spawned character's `AnimationPlayer` (added by the scene
/// loader on a descendant of the `SceneRoot`) to its per-letter graph, and mark
/// the owning [`CharModel`] so the drive system can transition it. Runs every
/// frame but only touches players added since last frame.
pub fn attach_char_animation(
    mut commands: Commands,
    assets: Res<CharAssets>,
    added: Query<Entity, Added<AnimationPlayer>>,
    parents: Query<&ChildOf>,
    mut models: Query<&mut CharModel>,
) {
    for player_ent in &added {
        // Walk up the hierarchy from the AnimationPlayer to its CharModel root.
        let mut cur = player_ent;
        let owner = loop {
            if models.contains(cur) {
                break Some(cur);
            }
            match parents.get(cur) {
                Ok(child_of) => cur = child_of.parent(),
                Err(_) => break None,
            }
        };
        let Some(owner) = owner else { continue };
        let Ok(mut model) = models.get_mut(owner) else {
            continue;
        };
        let Some(loaded) = assets.0.get(&model.letter) else {
            continue;
        };
        commands.entity(player_ent).insert((
            AnimationGraphHandle(loaded.graph.clone()),
            AnimationTransitions::new(),
        ));
        model.player = Some(player_ent);
        model.applied = None; // force the drive system to (re)apply the state
    }
}

/// Apply each character's desired [`CharModel::state`] to its `AnimationPlayer`,
/// cross-fading when it changed. No-op for characters whose player hasn't been
/// resolved yet (scene still loading).
pub fn drive_char_animation(
    assets: Res<CharAssets>,
    mut models: Query<&mut CharModel>,
    mut players: Query<(&mut AnimationPlayer, &mut AnimationTransitions)>,
) {
    for mut model in &mut models {
        if model.applied == Some(model.state) {
            continue;
        }
        let Some(player_ent) = model.player else {
            continue;
        };
        let Some(loaded) = assets.0.get(&model.letter) else {
            continue;
        };
        let Ok((mut player, mut transitions)) = players.get_mut(player_ent) else {
            continue;
        };
        let node = loaded.nodes[model.state as usize];
        transitions
            .play(&mut player, node, Duration::from_millis(180))
            .repeat();
        model.applied = Some(model.state);
    }
}

/// Advance the player shoot latch from the fire-cooldown rising edge. Called by
/// `sync_player`; returns the current locomotion-or-shoot state for the player.
pub fn player_anim_state(anim: &mut PlayerAnim, sim: &Sim, dt: f32, speed: f32) -> AnimClip {
    let cd = sim.0.player_fire_cooldown;
    // A fresh shot resets the cooldown upward — that rising edge is a fire.
    if cd > anim.prev_cooldown + 1e-4 {
        anim.shoot_ttl = SHOOT_HOLD;
    }
    anim.prev_cooldown = cd;
    anim.shoot_ttl = (anim.shoot_ttl - dt).max(0.0);

    if anim.shoot_ttl > 0.0 {
        AnimClip::Shoot
    } else {
        locomotion(speed, true)
    }
}
