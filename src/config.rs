//! Gameplay constants and pure helpers shared by client and server.
//!
//! NOTE: no `bevy`/`Vec2` here on purpose - this module must stay dependency-free
//! so both binaries compile against the exact same plain data.

pub const PROTOCOL_ID: u64 = 7;
pub const SERVER_ADDR: &str = "127.0.0.1:5000";

/// Server simulation rate. The server steps its world at this fixed rate and
/// broadcasts one snapshot per step.
pub const TICK_HZ: f64 = 30.0;
pub const DT: f32 = 1.0 / TICK_HZ as f32;
/// Max catch-up steps per frame after a stall (prevents spiral of death).
pub const MAX_STEPS: u32 = 5;

/// Half-extent of the playable field. Players are clamped to this (minus radius).
pub const FIELD_HALF: [f32; 2] = [800.0, 600.0];
pub const PLAYER_RADIUS: f32 = 24.0;
pub const PLAYER_SPEED: f32 = 240.0;
pub const PLAYER_MAX_HP: i32 = 100;
pub const SPAWN_POINT: [f32; 2] = [-300.0, 0.0];
pub const STARTING_SEEDS: i32 = 5;
pub const HP_REGEN_PER_SEC: f32 = 1.0;

/// Farm plot grid: `PLOT_COLS * PLOT_ROWS` plots, first plot at PLOT_ORIGIN.
pub const PLOT_ORIGIN: [f32; 2] = [250.0, -460.0];
pub const PLOT_SPACING: f32 = 80.0;
pub const PLOT_COLS: usize = 6;
pub const PLOT_ROWS: usize = 5;
pub const PLOT_SIZE: f32 = 64.0;
/// How close (world units) a player must be to interact with a plot.
pub const INTERACT_RANGE: f32 = 75.0;

/// Player attack ("Cheat Skill: One Punch").
pub const ATTACK_RANGE: f32 = 95.0;
pub const ATTACK_COOLDOWN: f32 = 0.4;
pub const ATTACK_DAMAGE: i32 = 150;

pub const ENEMY_CAP: usize = 10;
pub const ENEMY_SPAWN_INTERVAL: f32 = 4.0;
pub const ENEMY_CHASE_RANGE: f32 = 600.0;
pub const ENEMY_ATTACK_RANGE: f32 = 42.0;
pub const ENEMY_ATTACK_COOLDOWN: f32 = 1.0;
/// Kind 0 = Slime, kind 1 = Shadow Wolf.
pub fn enemy_hp(kind: u8) -> i32 {
    if kind == 0 { 100 } else { 250 }
}
pub fn enemy_speed(kind: u8) -> f32 {
    if kind == 0 { 90.0 } else { 145.0 }
}
pub fn enemy_damage(kind: u8) -> i32 {
    if kind == 0 { 8 } else { 16 }
}
pub fn enemy_name(kind: u8) -> &'static str {
    if kind == 0 { "Slime" } else { "Shadow Wolf" }
}

/// Seconds for a watered crop to reach full growth; dry crops grow at `CROP_DRY_MULT`.
pub const CROP_GROW_TIME: f32 = 20.0;
pub const CROP_DRY_MULT: f32 = 0.3;
pub const CROP_NAME: &str = "Divine Turnip";
/// Harvest yields; killing an enemy has this chance to drop a seed.
pub const SEEDS_PER_HARVEST: i32 = 2;
pub const SEED_DROP_CHANCE: f64 = 0.5;

/// World position of farm plot `i` (index = row-major, must match snapshot order).
pub fn plot_pos(i: usize) -> [f32; 2] {
    let col = i % PLOT_COLS;
    let row = i / PLOT_COLS;
    [
        PLOT_ORIGIN[0] + col as f32 * PLOT_SPACING,
        PLOT_ORIGIN[1] + row as f32 * PLOT_SPACING,
    ]
}

pub fn plot_count() -> usize {
    PLOT_COLS * PLOT_ROWS
}

/// Closest plot within INTERACT_RANGE of (x, y), if any.
pub fn nearest_plot(x: f32, y: f32) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for i in 0..plot_count() {
        let p = plot_pos(i);
        let d = (p[0] - x).hypot(p[1] - y);
        if d <= INTERACT_RANGE && best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Clamp a position inside the playable field (players use this on both sides).
pub fn clamp_to_field(x: f32, y: f32) -> [f32; 2] {
    let mx = FIELD_HALF[0] - PLAYER_RADIUS;
    let my = FIELD_HALF[1] - PLAYER_RADIUS;
    [x.clamp(-mx, mx), y.clamp(-my, my)]
}
