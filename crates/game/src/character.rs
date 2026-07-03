//! GLTF character rendering + animation — the payoff of the Bevy move
//! (CLAUDE.md "deferred epic"). Loads rigged GLTF character models through the
//! `AssetServer` and animates them from their own GLTF animation clips via
//! Bevy's animation graph.
//!
//! **Everything here is art-agnostic except one clearly-marked catalog.** The
//! systems (spawn / attach / drive / the reconcile in `scene.rs`, the lighting +
//! materials in `main.rs`/`scene.rs`) don't know or care *which* models are
//! loaded. To replace the current CC0 Kenney placeholders with final art you
//! touch only:
//!
//! 1. [`player_spec`] / [`enemy_spec`] — the model path + world-scale per role.
//! 2. [`CLIP_NAMES`] — the semantic-state → glb-clip-name map, *if* the new
//!    models name their animation clips differently.
//! 3. [`CHAR_BASE_SCALE`] / [`MODEL_FACING_OFFSET`] / [`CHAR_NATIVE_HEIGHT`] — the
//!    whole-set fit tuning (native model size / rest orientation).
//!
//! Clips are matched by **name**, not by index, and a model missing a clip
//! degrades gracefully (falls back to idle) — so a drop-in model needs no code
//! changes here, only a catalog edit.
//!
//! The reconcile (id-keyed pooling, Transform follow) lives in `scene.rs`; this
//! module owns the *visual*: what to spawn ([`spawn_character`]), the catalog,
//! and the animation state machine ([`CharModel`] + the graph/attach/drive
//! systems).

use crate::Sim;
use bevy::gltf::{Gltf, GltfAssetLabel};
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};
use std::collections::HashMap;
use std::time::Duration;

// ---- Whole-set fit tuning (retune once when swapping the art set) --------

/// Native model height → world-tile multiplier. The per-role scale in the
/// catalog is applied on top of this. Tuned so a scale-`1.0` model stands ~1
/// tile tall; a taller/shorter final-art set only needs this one number changed.
const CHAR_BASE_SCALE: f32 = 0.5;
/// Approx native (unscaled) model height, head-to-toe, in model units — used to
/// anchor health bars above a character. Set per art set.
const CHAR_NATIVE_HEIGHT: f32 = 2.0;
/// Radians added to a character's yaw so its rest-pose forward faces the gameplay
/// heading (front = +Z). `0.0` for models that already face +Z; `PI` if they
/// face −Z. One value for the whole art set.
const MODEL_FACING_OFFSET: f32 = 0.0;

// ---- Animation state machine tuning --------------------------------------

/// Ground speed (tiles/sec) above which a character plays its walk cycle.
const WALK_SPEED_THRESHOLD: f32 = 0.35;
/// Ground speed (tiles/sec) above which a character breaks into a run.
const SPRINT_SPEED_THRESHOLD: f32 = 3.2;
/// Distance (tiles) at which an awake enemy plays its melee-attack clip.
pub const ATTACK_RANGE: f32 = 0.9;
/// How long the player holds the shoot pose after a shot (seconds); each shot
/// re-arms it, so continuous fire stays in the shoot animation.
const SHOOT_HOLD: f32 = 0.28;
/// Animation cross-fade time between states.
const CROSSFADE: Duration = Duration::from_millis(180);

// ---- Placeholder character catalog (THE swap seam) -----------------------
//
// CC0 Kenney "Blocky Characters" (see assets/characters/kennys/CREDITS.txt),
// chosen by rough vibe (tank = bulky, boss = big). Everything below is pure data
// — model paths are `&'static str` (no per-frame allocation) and scales are
// relative sizes. Replace these with final-art paths/scales; nothing else here
// changes.

/// One character role's art: the glb to spawn and its relative world-scale.
#[derive(Clone, Copy)]
pub struct CharSpec {
    /// Asset path of the glb (relative to the asset root).
    pub model: &'static str,
    /// Relative world-scale, multiplied by [`CHAR_BASE_SCALE`].
    pub scale: f32,
}

/// The player's model + scale. Distinct look so the player reads apart from the
/// swarm.
pub fn player_spec() -> CharSpec {
    CharSpec {
        model: "characters/kennys/character-a.glb",
        scale: 1.05,
    }
}

/// The model + scale for an enemy archetype id. Unknown ids fall back to the
/// basic-zombie look.
pub fn enemy_spec(archetype_id: &str) -> CharSpec {
    let (model, scale) = match archetype_id {
        "swarm_rusher" => ("characters/kennys/character-n.glb", 0.8),
        "fast_zombie" => ("characters/kennys/character-h.glb", 0.9),
        "spitter" => ("characters/kennys/character-k.glb", 1.0),
        "basic_zombie" => ("characters/kennys/character-c.glb", 1.0),
        "fat_zombie" => ("characters/kennys/character-m.glb", 1.35),
        "patient_zero" => ("characters/kennys/character-r.glb", 1.9),
        _ => ("characters/kennys/character-c.glb", 1.0),
    };
    CharSpec { model, scale }
}

/// Every archetype id the enemy catalog knows — drives startup preloading (with
/// the player) so exactly the referenced models load.
const ENEMY_ARCHETYPES: [&str; 6] = [
    "swarm_rusher",
    "fast_zombie",
    "spitter",
    "basic_zombie",
    "fat_zombie",
    "patient_zero",
];

/// Semantic animation → the glb clip *name* to bind to it. Matched by name (not
/// index) so any model exporting these conventional clip names drops in; a model
/// missing one degrades to idle for that state (see [`drive_char_animation`]).
/// The right-hand names are the only thing to touch if final art names its clips
/// differently.
const CLIP_NAMES: [(AnimClip, &str); 7] = [
    (AnimClip::Idle, "idle"),
    (AnimClip::Walk, "walk"),
    (AnimClip::Sprint, "sprint"),
    (AnimClip::HoldBoth, "holding-both"),
    (AnimClip::Shoot, "holding-both-shoot"),
    (AnimClip::Attack, "attack-melee-right"),
    (AnimClip::Die, "die"),
];

// ---- Animation states ----------------------------------------------------

/// The semantic animation states the game drives. Each maps to a glb clip by
/// name via [`CLIP_NAMES`]; the index (`as usize`) keys into a loaded model's
/// per-clip node table.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AnimClip {
    Idle = 0,
    Walk = 1,
    Sprint = 2,
    HoldBoth = 3,
    Shoot = 4,
    Attack = 5,
    /// Reserved for a death animation once the sim keeps a dying corpse around;
    /// no trigger trips it yet.
    #[allow(dead_code)]
    Die = 6,
}

/// Number of [`AnimClip`] states — the size of each model's per-clip node table.
const N_CLIPS: usize = 7;

// ---- Loaded-asset store --------------------------------------------------

/// A loaded character model: its scene (a GLTF `WorldAsset`), the `Gltf` asset
/// (read once to build the graph by clip name), and — once built — an
/// `AnimationGraph` with a node per [`AnimClip`] (`None` where the model lacks
/// that clip). `graph` stays `None` until the async `Gltf` load finishes.
struct LoadedChar {
    scene: Handle<WorldAsset>,
    gltf: Handle<Gltf>,
    graph: Option<Handle<AnimationGraph>>,
    nodes: [Option<AnimationNodeIndex>; N_CLIPS],
}

/// Loaded character models, keyed by their glb path. Built once at startup for
/// exactly the models the catalog references.
#[derive(Resource, Default)]
pub struct CharAssets(HashMap<&'static str, LoadedChar>);

/// Render-side player shoot latch: tracks the fire-cooldown rising edge to drive
/// the shoot animation without any sim change (see [`crate::scene::sync_player`]).
#[derive(Resource, Default)]
pub struct PlayerAnim {
    pub prev_cooldown: f32,
    pub shoot_ttl: f32,
}

/// Marks a spawned character's `WorldAssetRoot` and carries its desired animation
/// state. The reconcile writes `state` each frame; [`drive_char_animation`]
/// applies transitions when it changes. `player` is the descendant
/// `AnimationPlayer` entity (resolved by [`attach_char_animation`]); `attached`
/// flips once its graph is bound.
#[derive(Component)]
pub struct CharModel {
    pub model: &'static str,
    pub state: AnimClip,
    applied: Option<AnimClip>,
    player: Option<Entity>,
    attached: bool,
}

impl CharModel {
    fn new(model: &'static str, state: AnimClip) -> Self {
        Self {
            model,
            state,
            applied: None,
            player: None,
            attached: false,
        }
    }
}

// ---- Catalog-derived helpers ---------------------------------------------

/// Approximate world-space height of a character at `scale` — where a health bar
/// anchors above it.
pub fn char_height(scale: f32) -> f32 {
    CHAR_NATIVE_HEIGHT * CHAR_BASE_SCALE * scale
}

/// The world transform for a character: ground position, heading yaw (+ the
/// art-set facing offset), and world scale.
pub fn char_transform(pos: Vec3, yaw: f32, scale: f32) -> Transform {
    Transform::from_translation(pos)
        .with_rotation(Quat::from_rotation_y(yaw + MODEL_FACING_OFFSET))
        .with_scale(Vec3::splat(CHAR_BASE_SCALE * scale))
}

/// Pick the idle/walk/sprint animation for a character moving at `speed`
/// (tiles/sec). `armed` characters (the player) idle in the gun-holding pose so
/// the weapon shows; enemies use the plain idle.
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

// ---- Systems -------------------------------------------------------------

/// Load the scene + `Gltf` for every model the catalog references (player + enemy
/// roster), deduped by path. Runs once at startup; the loads are async — the
/// graph is built later by [`build_char_graphs`] once each `Gltf` resolves.
pub fn setup_characters(asset_server: Res<AssetServer>, mut store: ResMut<CharAssets>) {
    let mut models: HashSet<&'static str> = HashSet::default();
    models.insert(player_spec().model);
    for id in ENEMY_ARCHETYPES {
        models.insert(enemy_spec(id).model);
    }
    for model in models {
        let scene = asset_server.load(GltfAssetLabel::Scene(0).from_asset(model));
        let gltf = asset_server.load::<Gltf>(model);
        store.0.insert(
            model,
            LoadedChar {
                scene,
                gltf,
                graph: None,
                nodes: [None; N_CLIPS],
            },
        );
    }
}

/// Once a model's `Gltf` has loaded, build its `AnimationGraph` by binding each
/// [`CLIP_NAMES`] clip *by name* — so the graph is composed of exactly the
/// semantic states the model actually provides. Missing clips leave a `None`
/// node (handled at play time). Runs each frame until every model's graph exists.
pub fn build_char_graphs(
    gltfs: Res<Assets<Gltf>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut store: ResMut<CharAssets>,
) {
    for loaded in store.0.values_mut() {
        if loaded.graph.is_some() {
            continue;
        }
        let Some(gltf) = gltfs.get(&loaded.gltf) else {
            continue; // still loading
        };
        let mut graph = AnimationGraph::new();
        let root = graph.root;
        let mut nodes = [None; N_CLIPS];
        for (clip, name) in CLIP_NAMES {
            if let Some(handle) = gltf.named_animations.get(name) {
                nodes[clip as usize] = Some(graph.add_clip(handle.clone(), 1.0, root));
            }
        }
        loaded.nodes = nodes;
        loaded.graph = Some(graphs.add(graph));
    }
}

/// Spawn a character model for `spec` at ground `pos` facing `yaw`, in animation
/// `state`. Returns the root entity so the caller tags it (`EnemyView` /
/// `PlayerView`); the animation wires itself up once the scene's
/// `AnimationPlayer` appears and the model's graph is built.
pub fn spawn_character(
    commands: &mut Commands,
    assets: &CharAssets,
    spec: CharSpec,
    pos: Vec3,
    yaw: f32,
    state: AnimClip,
) -> Entity {
    let scene = assets
        .0
        .get(spec.model)
        .map(|c| c.scene.clone())
        .unwrap_or_default();
    commands
        .spawn((
            WorldAssetRoot(scene),
            char_transform(pos, yaw, spec.scale),
            CharModel::new(spec.model, state),
        ))
        .id()
}

/// Resolve each freshly-spawned character's `AnimationPlayer` (added by the scene
/// loader on a descendant of the model root) to its owning [`CharModel`], and
/// give it an `AnimationTransitions` so [`drive_char_animation`] can cross-fade
/// it. Runs every frame but only touches players added since last frame.
pub fn attach_char_animation(
    mut commands: Commands,
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
        model.player = Some(player_ent);
        commands
            .entity(player_ent)
            .insert(AnimationTransitions::new());
    }
}

/// Bind each character's graph to its player once both are ready, then apply the
/// desired [`CharModel::state`], cross-fading on change. A state whose clip the
/// model lacks falls back to idle. No-op until the player is resolved and the
/// graph built (scene / `Gltf` still loading).
pub fn drive_char_animation(
    mut commands: Commands,
    assets: Res<CharAssets>,
    mut models: Query<&mut CharModel>,
    mut players: Query<(&mut AnimationPlayer, &mut AnimationTransitions)>,
) {
    for mut model in &mut models {
        let Some(player_ent) = model.player else {
            continue;
        };
        let Some(loaded) = assets.0.get(model.model) else {
            continue;
        };
        let Some(graph) = loaded.graph.clone() else {
            continue; // graph not built yet
        };
        // First frame the graph is ready: bind it to the player.
        if !model.attached {
            commands
                .entity(player_ent)
                .insert(AnimationGraphHandle(graph));
            model.attached = true;
            model.applied = None;
        }
        if model.applied == Some(model.state) {
            continue;
        }
        // Resolve the clip node, falling back to idle if this model lacks it.
        let Some(node) =
            loaded.nodes[model.state as usize].or(loaded.nodes[AnimClip::Idle as usize])
        else {
            continue;
        };
        let Ok((mut player, mut transitions)) = players.get_mut(player_ent) else {
            continue; // AnimationTransitions insert not flushed yet; retry next frame
        };
        transitions.play(&mut player, node, CROSSFADE).repeat();
        model.applied = Some(model.state);
    }
}
