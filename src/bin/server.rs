//! Headless authoritative server.
//!
//! Runs a fixed 30 Hz simulation (manual accumulator in `server_update`, NOT
//! Bevy's FixedUpdate), receives client inputs, and broadcasts snapshots.
//! No rendering: MinimalPlugins only.

use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::{Duration, SystemTime};

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use bevy_renet::netcode::{
    NetcodeServerPlugin, NetcodeServerTransport, ServerAuthentication, ServerConfig,
};
use bevy_renet::renet::{ClientId, ConnectionConfig, DefaultChannel, ServerEvent};
use bevy_renet::{RenetServer, RenetServerEvent, RenetServerPlugin};
use rand::{thread_rng, Rng};

use justin_rpg::config as cfg;
use justin_rpg::protocol::{
    ActionKind, C2S, CropSnap, EnemySnap, PlotSnap, PlayerSnap, S2C,
};

fn main() {
    println!("JustinRPG server starting on {}/UDP ...", cfg::SERVER_ADDR);

    let (server, transport) = new_renet_server();

    App::new()
        .add_plugins(
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(16))),
        )
        .add_plugins(RenetServerPlugin)
        .add_plugins(NetcodeServerPlugin)
        .insert_resource(server)
        .insert_resource(transport)
        .init_resource::<World>()
        .init_resource::<Accum>()
        .add_systems(Startup, setup)
        .add_systems(Update, server_update)
        .add_observer(on_client_event)
        .run();
}

fn new_renet_server() -> (RenetServer, NetcodeServerTransport) {
    let public_addr = cfg::SERVER_ADDR.parse().unwrap();
    let socket = UdpSocket::bind(public_addr).unwrap();
    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let server_config = ServerConfig {
        current_time,
        max_clients: 16,
        protocol_id: cfg::PROTOCOL_ID,
        public_addresses: vec![public_addr],
        authentication: ServerAuthentication::Unsecure,
    };
    let transport = NetcodeServerTransport::new(server_config, socket).unwrap();
    let server = RenetServer::new(ConnectionConfig::default());
    (server, transport)
}

fn setup() {
    println!(
        "World ready: {} farm plots, {} Hz tick, field {:?}.",
        cfg::plot_count(),
        cfg::TICK_HZ,
        cfg::FIELD_HALF
    );
}

// ---------------------------------------------------------------------------
// Server-side authoritative state
// ---------------------------------------------------------------------------

struct Player {
    pos: [f32; 2],
    hp: i32,
    seeds: i32,
    harvests: i32,
    kills: u32,
    /// Latest received movement direction; sticky (client sends zeros when idle).
    move_input: [f32; 2],
    attack_cd: f32,
    regen_acc: f32,
}

impl Player {
    fn new(pos: [f32; 2]) -> Self {
        Self {
            pos,
            hp: cfg::PLAYER_MAX_HP,
            seeds: cfg::STARTING_SEEDS,
            harvests: 0,
            kills: 0,
            move_input: [0.0, 0.0],
            attack_cd: 0.0,
            regen_acc: 0.0,
        }
    }
}

struct Enemy {
    id: u32,
    kind: u8,
    pos: [f32; 2],
    hp: i32,
    hit_cd: f32,
}

#[derive(Clone, Copy)]
struct Crop {
    growth: f32,
    watered: bool,
}

#[derive(Clone, Copy, Default)]
struct Plot {
    tilled: bool,
    crop: Option<Crop>,
}

#[derive(Resource)]
struct World {
    players: HashMap<ClientId, Player>,
    enemies: Vec<Enemy>,
    plots: Vec<Plot>,
    next_enemy_id: u32,
    spawn_timer: f32,
    tick: u64,
    /// Log lines queued for broadcast (flushed every update/tick).
    logs: Vec<String>,
}

impl Default for World {
    fn default() -> Self {
        Self {
            players: HashMap::new(),
            enemies: Vec::new(),
            plots: vec![Plot::default(); cfg::plot_count()],
            next_enemy_id: 0,
            spawn_timer: cfg::ENEMY_SPAWN_INTERVAL,
            tick: 0,
            logs: Vec::new(),
        }
    }
}

#[derive(Resource, Default)]
struct Accum(f32);

fn hero_name(client_id: ClientId) -> String {
    format!("Hero-{:04}", client_id % 10000)
}

// ---------------------------------------------------------------------------
// Connection events
// ---------------------------------------------------------------------------

fn on_client_event(
    event: On<RenetServerEvent>,
    mut world: ResMut<World>,
    mut server: ResMut<RenetServer>,
) {
    match &**event {
        ServerEvent::ClientConnected { client_id } => {
            let client_id = *client_id;
            let idx = world.players.len();
            // Small spawn fan-out so co-joining players don't stack.
            let pos = cfg::clamp_to_field(
                cfg::SPAWN_POINT[0] + (idx % 5) as f32 * 56.0 - 112.0,
                cfg::SPAWN_POINT[1] + (idx / 5) as f32 * 56.0,
            );
            world.players.insert(client_id, Player::new(pos));
            println!("{} connected (id {client_id}).", hero_name(client_id));
            world.logs.push(format!(
                "{} has been reincarnated into this world.",
                hero_name(client_id)
            ));
            // Onboarding only to the joining client.
            let welcome = S2C::Log {
                text: "Welcome, OP protagonist! WASD move, SPACE one-punches, \
                       E tends the nearest farm plot (till -> plant -> water -> harvest)."
                    .to_string(),
            };
            server.send_message(
                client_id,
                DefaultChannel::ReliableOrdered,
                bincode::serialize(&welcome).unwrap(),
            );
        }
        ServerEvent::ClientDisconnected { client_id, reason } => {
            let client_id = *client_id;
            world.players.remove(&client_id);
            println!(
                "{} disconnected ({reason}).",
                hero_name(client_id)
            );
            world.logs.push(format!(
                "{} returned to their original world.",
                hero_name(client_id)
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Main update: receive -> step sim at fixed rate -> snapshot -> logs
// ---------------------------------------------------------------------------

fn server_update(
    time: Res<Time>,
    mut server: ResMut<RenetServer>,
    mut world: ResMut<World>,
    mut accum: ResMut<Accum>,
) {
    receive_inputs(&mut server, &mut world);
    flush_logs(&mut server, &mut world);

    // Guard against huge deltas after stalls.
    accum.0 += time.delta().as_secs_f32().min(0.25);
    let mut steps = 0u32;
    while accum.0 >= cfg::DT && steps < cfg::MAX_STEPS {
        step(&mut world);
        accum.0 -= cfg::DT;
        steps += 1;
    }
    if steps == cfg::MAX_STEPS {
        accum.0 = 0.0;
    }
    if steps > 0 {
        broadcast_snapshot(&mut server, &world);
        flush_logs(&mut server, &mut world);
    }
}

fn receive_inputs(server: &mut RenetServer, world: &mut World) {
    for client_id in server.clients_id() {
        // Movement: unreliable, last-writer-wins.
        while let Some(bytes) = server.receive_message(client_id, DefaultChannel::Unreliable) {
            if let Ok(C2S::Move { dir }) = bincode::deserialize(&bytes) {
                if let Some(player) = world.players.get_mut(&client_id) {
                    player.move_input = [dir[0].clamp(-1.0, 1.0), dir[1].clamp(-1.0, 1.0)];
                }
            }
        }
        // Actions: reliable, processed immediately (cooldowns live in World).
        while let Some(bytes) = server.receive_message(client_id, DefaultChannel::ReliableOrdered) {
            if let Ok(C2S::Action(kind)) = bincode::deserialize(&bytes) {
                handle_action(world, client_id, kind);
            }
        }
    }
}

fn handle_action(world: &mut World, client_id: ClientId, kind: ActionKind) {
    match kind {
        ActionKind::Attack => {
            let Some(player) = world.players.get_mut(&client_id) else {
                return;
            };
            if player.attack_cd > 0.0 {
                return;
            }
            player.attack_cd = cfg::ATTACK_COOLDOWN;
            let origin = player.pos;

            let range2 = cfg::ATTACK_RANGE * cfg::ATTACK_RANGE;
            let hit: Vec<u32> = world
                .enemies
                .iter()
                .filter(|e| {
                    let dx = e.pos[0] - origin[0];
                    let dy = e.pos[1] - origin[1];
                    dx * dx + dy * dy <= range2
                })
                .map(|e| e.id)
                .collect();
            if hit.is_empty() {
                return;
            }
            for e in world.enemies.iter_mut().filter(|e| hit.contains(&e.id)) {
                e.hp -= cfg::ATTACK_DAMAGE;
            }

            let mut rng = thread_rng();
            let attacker = hero_name(client_id);
            let mut survivors = Vec::with_capacity(world.enemies.len());
            for e in world.enemies.drain(..) {
                if e.hp > 0 {
                    survivors.push(e);
                    continue;
                }
                let mut line = format!(
                    "{attacker} obliterated a {} with [Cheat Skill: One Punch]!",
                    cfg::enemy_name(e.kind)
                );
                if let Some(p) = world.players.get_mut(&client_id) {
                    p.kills += 1;
                    if rng.gen_bool(cfg::SEED_DROP_CHANCE) {
                        p.seeds += 1;
                        line.push_str(" It dropped a seed!");
                    }
                }
                world.logs.push(line);
            }
            world.enemies = survivors;
        }
        ActionKind::Farm => {
            let Some(player) = world.players.get_mut(&client_id) else {
                return;
            };
            let Some(pi) = cfg::nearest_plot(player.pos[0], player.pos[1]) else {
                return;
            };
            // `world.players` and `world.plots` are disjoint fields: both can be
            // mutably borrowed here.
            let plot = &mut world.plots[pi];
            let name = hero_name(client_id);

            if !plot.tilled {
                plot.tilled = true;
                world
                    .logs
                    .push(format!("{name} tilled a plot of soil."));
            } else if plot.crop.is_none() {
                if player.seeds > 0 {
                    player.seeds -= 1;
                    plot.crop = Some(Crop {
                        growth: 0.0,
                        watered: false,
                    });
                    world.logs.push(format!(
                        "{name} planted a {} seed. Water it with E!",
                        cfg::CROP_NAME
                    ));
                } else {
                    world.logs.push(format!(
                        "{name} has no seeds. Harvest ripe crops (or slay monsters) for more."
                    ));
                }
            } else if let Some(crop) = plot.crop.as_mut() {
                if crop.growth >= 1.0 {
                    plot.crop = None;
                    player.seeds += cfg::SEEDS_PER_HARVEST;
                    player.harvests += 1;
                    world.logs.push(format!(
                        "{name} harvested a {}! (+{} seeds)",
                        cfg::CROP_NAME,
                        cfg::SEEDS_PER_HARVEST
                    ));
                } else if !crop.watered {
                    crop.watered = true;
                    world
                        .logs
                        .push(format!("{name} watered a crop. It will grow quickly now."));
                }
            }
        }
    }
}

fn step(world: &mut World) {
    world.tick += 1;
    let dt = cfg::DT;

    // --- players: move, cooldowns, regen, deaths ---
    let mut deaths = Vec::new();
    for (id, p) in world.players.iter_mut() {
        let (dx, dy) = (p.move_input[0], p.move_input[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len > 1.0 {
            p.pos = cfg::clamp_to_field(
                p.pos[0] + dx / len * cfg::PLAYER_SPEED * dt,
                p.pos[1] + dy / len * cfg::PLAYER_SPEED * dt,
            );
        } else {
            p.pos = cfg::clamp_to_field(
                p.pos[0] + dx * cfg::PLAYER_SPEED * dt,
                p.pos[1] + dy * cfg::PLAYER_SPEED * dt,
            );
        }

        p.attack_cd = (p.attack_cd - dt).max(0.0);
        p.regen_acc += dt * cfg::HP_REGEN_PER_SEC;
        while p.regen_acc >= 1.0 {
            p.hp = (p.hp + 1).min(cfg::PLAYER_MAX_HP);
            p.regen_acc -= 1.0;
        }
        if p.hp <= 0 {
            deaths.push(*id);
        }
    }
    for id in deaths {
        if let Some(p) = world.players.get_mut(&id) {
            p.hp = cfg::PLAYER_MAX_HP;
            p.regen_acc = 0.0;
            p.pos = cfg::clamp_to_field(cfg::SPAWN_POINT[0], cfg::SPAWN_POINT[1]);
        }
        world.logs.push(format!(
            "{} was flattened by a monster... embarrassing for an OP protagonist. \
             Reincarnated at the shrine.",
            hero_name(id)
        ));
    }

    // --- enemies: chase nearest player, attack, cooldowns ---
    for i in 0..world.enemies.len() {
        let (kind, epos) = {
            let e = &world.enemies[i];
            (e.kind, e.pos)
        };

        // Nearest player within chase range.
        let mut target: Option<[f32; 2]> = None;
        let mut best = cfg::ENEMY_CHASE_RANGE * cfg::ENEMY_CHASE_RANGE;
        for p in world.players.values() {
            let dx = p.pos[0] - epos[0];
            let dy = p.pos[1] - epos[1];
            let d2 = dx * dx + dy * dy;
            if d2 < best {
                best = d2;
                target = Some(p.pos);
            }
        }

        if let Some(tpos) = target {
            let dx = tpos[0] - epos[0];
            let dy = tpos[1] - epos[1];
            let d = (dx * dx + dy * dy).sqrt();
            if d > 0.001 {
                let speed = cfg::enemy_speed(kind);
                let enemy = &mut world.enemies[i];
                enemy.pos[0] += dx / d * speed * dt;
                enemy.pos[1] += dy / d * speed * dt;
            }
            // Attack if in contact range and off cooldown.
            let reach = cfg::ENEMY_ATTACK_RANGE + cfg::PLAYER_RADIUS;
            if d <= reach {
                let enemy = &mut world.enemies[i];
                if enemy.hit_cd <= 0.0 {
                    enemy.hit_cd = cfg::ENEMY_ATTACK_COOLDOWN;
                    // Disjoint field borrow: enemies vs players.
                    if let Some(nearest) = world
                        .players
                        .values_mut()
                        .filter(|p| {
                            let dx = p.pos[0] - epos[0];
                            let dy = p.pos[1] - epos[1];
                            dx * dx + dy * dy <= reach * reach
                        })
                        .min_by(|a, b| {
                            let ax = a.pos[0] - epos[0];
                            let ay = a.pos[1] - epos[1];
                            let bx = b.pos[0] - epos[0];
                            let by = b.pos[1] - epos[1];
                            (ax * ax + ay * ay)
                                .partial_cmp(&(bx * bx + by * by))
                                .unwrap()
                        })
                    {
                        nearest.hp -= cfg::enemy_damage(kind);
                    }
                }
            }
        }
        // hit cooldown tick
        let enemy = &mut world.enemies[i];
        enemy.hit_cd = (enemy.hit_cd - dt).max(0.0);
    }

    // --- enemy separation (so they don't fully stack) ---
    let n = world.enemies.len();
    let mut push = vec![[0.0f32, 0.0f32]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let (pi, pj) = (world.enemies[i].pos, world.enemies[j].pos);
            let dx = pi[0] - pj[0];
            let dy = pi[1] - pj[1];
            let d2 = dx * dx + dy * dy;
            const MIN_D: f32 = 34.0;
            if d2 < MIN_D * MIN_D && d2 > 0.0001 {
                let d = d2.sqrt();
                let f = (MIN_D - d) / d * 0.5;
                push[i][0] += dx * f;
                push[i][1] += dy * f;
                push[j][0] -= dx * f;
                push[j][1] -= dy * f;
            }
        }
    }
    for (e, pv) in world.enemies.iter_mut().zip(push) {
        e.pos[0] += pv[0];
        e.pos[1] += pv[1];
    }

    // --- spawner ---
    world.spawn_timer -= dt;
    if world.spawn_timer <= 0.0 {
        world.spawn_timer = cfg::ENEMY_SPAWN_INTERVAL;
        if world.enemies.len() < cfg::ENEMY_CAP {
            let mut rng = thread_rng();
            let kind: u8 = if rng.gen_bool(0.75) { 0 } else { 1 };
            let pos = random_edge_pos(&mut rng);
            world.enemies.push(Enemy {
                id: world.next_enemy_id,
                kind,
                pos,
                hp: cfg::enemy_hp(kind),
                hit_cd: 0.0,
            });
            world.next_enemy_id += 1;
        }
    }

    // --- crops ---
    for plot in world.plots.iter_mut() {
        if let Some(crop) = plot.crop.as_mut() {
            let mult = if crop.watered { 1.0 } else { cfg::CROP_DRY_MULT };
            crop.growth = (crop.growth + dt / cfg::CROP_GROW_TIME * mult).min(1.0);
        }
    }
}

fn random_edge_pos(rng: &mut impl Rng) -> [f32; 2] {
    let (fx, fy) = (cfg::FIELD_HALF[0] - 24.0, cfg::FIELD_HALF[1] - 24.0);
    match rng.gen_range(0..4) {
        0 => [rng.gen_range(-fx..fx), -fy],
        1 => [rng.gen_range(-fx..fx), fy],
        2 => [-fx, rng.gen_range(-fy..fy)],
        _ => [fx, rng.gen_range(-fy..fy)],
    }
}

fn broadcast_snapshot(server: &mut RenetServer, world: &World) {
    let msg = S2C::Snapshot {
        tick: world.tick,
        players: world
            .players
            .iter()
            .map(|(id, p)| PlayerSnap {
                id: *id,
                pos: p.pos,
                hp: p.hp,
                seeds: p.seeds,
                harvests: p.harvests,
                kills: p.kills,
            })
            .collect(),
        enemies: world
            .enemies
            .iter()
            .map(|e| EnemySnap {
                id: e.id,
                kind: e.kind,
                pos: e.pos,
                hp: e.hp,
            })
            .collect(),
        plots: world
            .plots
            .iter()
            .map(|p| PlotSnap {
                tilled: p.tilled,
                crop: p.crop.map(|c| CropSnap {
                    growth: c.growth,
                    watered: c.watered,
                }),
            })
            .collect(),
    };
    server.broadcast_message(
        DefaultChannel::Unreliable,
        bincode::serialize(&msg).unwrap(),
    );
}

fn flush_logs(server: &mut RenetServer, world: &mut World) {
    if world.logs.is_empty() {
        return;
    }
    for text in world.logs.drain(..) {
        let msg = S2C::Log { text };
        server.broadcast_message(
            DefaultChannel::ReliableOrdered,
            bincode::serialize(&msg).unwrap(),
        );
    }
}
