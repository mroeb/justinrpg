//! Network protocol: client->server (`C2S`) and server->client (`S2C`).
//!
//! Serialized with `bincode` (version-sensitive: both sides are this same crate,
//! so never mix in an independently-built client/server).
//!
//! Channel usage:
//! - `DefaultChannel::Unreliable`: `C2S::Move` (every frame), `S2C::Snapshot` (30 Hz)
//! - `DefaultChannel::ReliableOrdered`: `C2S::Action`, `S2C::Log`

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum C2S {
    /// Latest movement direction ([-1,1] per axis, client-normalized), sticky until replaced.
    Move { dir: [f32; 2] },
    /// Edge-triggered actions must go on the reliable channel so they can't be dropped.
    Action(ActionKind),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ActionKind {
    /// Melee AoE one-punch (server checks cooldown/range).
    Attack,
    /// Context action on the nearest plot: till -> plant -> water -> harvest.
    Farm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum S2C {
    Snapshot {
        tick: u64,
        /// Includes the receiving player; key it by `id`.
        players: Vec<PlayerSnap>,
        enemies: Vec<EnemySnap>,
        /// Row-major order, same indexing as `config::plot_pos`.
        plots: Vec<PlotSnap>,
    },
    /// Flavor/system chat line (join, harvest, kills, onboarding).
    Log { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerSnap {
    pub id: u64,
    pub pos: [f32; 2],
    pub hp: i32,
    pub seeds: i32,
    pub harvests: i32,
    pub kills: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnemySnap {
    pub id: u32,
    pub kind: u8,
    pub pos: [f32; 2],
    pub hp: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlotSnap {
    pub tilled: bool,
    pub crop: Option<CropSnap>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CropSnap {
    /// 0.0..=1.0; >= 1.0 is harvestable.
    pub growth: f32,
    pub watered: bool,
}
