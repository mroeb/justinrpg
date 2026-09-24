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
    ActionKind, C2S, CropSnap, EnemySnap, NodeSnap, PlotSnap, PlayerSnap, S2C,
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
        "World ready: {} farm plots, {} resource nodes, {} regions, {} Hz tick, field {:?}.",
        cfg::plot_count(),
        cfg::node_count(),
        cfg::REGIONS.len(),
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
    // Inventory (resources + consumables).
    wood: i32,
    stone: i32,
    ore: i32,
    turnips: i32,
    potions: i32,
    /// Owned equipment as a bitmask of `cfg::GEAR_*` bits.
    gear: u8,
    /// Region index this player was last in (for entry announcements).
    region: u8,
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
            wood: 0,
            stone: 0,
            ore: 0,
            turnips: 0,
            potions: 0,
            gear: 0,
            region: cfg::region_index(pos[0], pos[1]) as u8,
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

/// Server-side state of a gatherable resource node (`cfg::NODE_DEFS` index).
#[derive(Clone, Copy)]
struct NodeState {
    ready: bool,
    timer: f32,
}

#[derive(Resource)]
struct World {
    players: HashMap<ClientId, Player>,
    enemies: Vec<Enemy>,
    plots: Vec<Plot>,
    nodes: Vec<NodeState>,
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
            nodes: vec![
                NodeState {
                    ready: true,
                    timer: 0.0,
                };
                cfg::node_count()
            ],
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
                       E tends the nearest farm plot, G gathers wood/stone/ore, \
                       Q drinks a Healing Draught, and 1-6 craft at the workbench. \
                       The Sanctuary at spawn is a safe zone: monsters cannot enter \
                       and your wounds close fast."
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
            let gear = player.gear;
            let damage = cfg::attack_damage(gear);

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
                e.hp -= damage;
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
                    // Drops by enemy kind: slime/wolf drop seeds, golems smash into ore.
                    if e.kind == 2 {
                        if rng.gen_bool(0.7) {
                            p.ore += 2;
                            line.push_str(" It shattered into 2 ore!");
                        }
                    } else if rng.gen_bool(cfg::SEED_DROP_CHANCE) {
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
                    let seeds = cfg::harvest_seeds(player.gear);
                    let turnips = cfg::harvest_turnips(player.gear);
                    player.seeds += seeds;
                    player.turnips += turnips;
                    player.harvests += 1;
                    world.logs.push(format!(
                        "{name} harvested a {}! (+{seeds} seeds, +{turnips} turnips)",
                        cfg::CROP_NAME
                    ));
                } else if !crop.watered {
                    crop.watered = true;
                    world
                        .logs
                        .push(format!("{name} watered a crop. It will grow quickly now."));
                }
            }
        }
        ActionKind::Gather => {
            let Some(player) = world.players.get_mut(&client_id) else {
                return;
            };
            let name = hero_name(client_id);
            let Some(ni) = cfg::nearest_node(player.pos[0], player.pos[1]) else {
                world.logs.push(
                    format!("{name} sees nothing to gather here. \
                             Look for trees in the Whisperwood, boulders in the Quarry, \
                             or ore veins in the Emberveins.")
                );
                return;
            };
            if !world.nodes[ni].ready {
                world.logs.push(format!(
                    "{name} found a depleted {}; it will regrow soon.",
                    cfg::NODE_NAMES[cfg::node_kind(ni) as usize]
                ));
                return;
            }
            let kind = cfg::node_kind(ni) as usize;
            let amount = cfg::GATHER_AMOUNT;
            world.nodes[ni].ready = false;
            world.nodes[ni].timer = cfg::NODE_RESPAWN_TIME;
            match kind as u8 {
                cfg::NODE_WOOD => player.wood += amount,
                cfg::NODE_STONE => player.stone += amount,
                _ => player.ore += amount,
            }
            world.logs.push(format!(
                "{name} gathered {amount} {} from a {}!",
                cfg::NODE_RESOURCE_NAMES[kind],
                cfg::NODE_NAMES[kind]
            ));
        }
        ActionKind::Craft(recipe_id) => {
            let Some(recipe) = cfg::RECIPES.get(recipe_id as usize).copied() else {
                return;
            };
            let Some(player) = world.players.get_mut(&client_id) else {
                return;
            };
            let name = hero_name(client_id);
            if recipe.gear != 0 && player.gear & recipe.gear != 0 {
                world
                    .logs
                    .push(format!("{name} already owns a {}.", recipe.name));
                return;
            }
            if !cfg::can_afford(&recipe, player.wood, player.stone, player.ore, player.turnips) {
                world.logs.push(format!(
                    "{name} lacks materials for {} ({}) - gather more or harvest turnips.",
                    recipe.name,
                    cfg::cost_text(&recipe)
                ));
                return;
            }
            player.wood -= recipe.wood;
            player.stone -= recipe.stone;
            player.ore -= recipe.ore;
            player.turnips -= recipe.turnips;
            if recipe.gear != 0 {
                let old_max = cfg::max_hp(player.gear);
                player.gear |= recipe.gear;
                let new_max = cfg::max_hp(player.gear);
                // New max HP grants the difference immediately (Titan Belt feel).
                player.hp = (player.hp + (new_max - old_max)).min(new_max);
                world.logs.push(format!(
                    "{name} crafted a {}! ({})",
                    recipe.name, recipe.blurb
                ));
            } else {
                player.potions += 1;
                world.logs.push(format!(
                    "{name} brewed a {}! ({})",
                    recipe.name, recipe.blurb
                ));
            }
        }
        ActionKind::Drink => {
            let Some(player) = world.players.get_mut(&client_id) else {
                return;
            };
            let name = hero_name(client_id);
            if player.potions <= 0 {
                world.logs.push(format!(
                    "{name} has no Healing Draughts - craft one with key 6 (1w 1o 1t)."
                ));
                return;
            }
            let max = cfg::max_hp(player.gear);
            if player.hp >= max {
                world.logs.push(format!("{name} is already at full HP."));
                return;
            }
            player.potions -= 1;
            let before = player.hp;
            player.hp = (player.hp + cfg::POTION_HEAL).min(max);
            world.logs.push(format!(
                "{name} quaffed a Healing Draught! (+{} HP)",
                player.hp - before
            ));
        }
    }
}

fn step(world: &mut World) {
    world.tick += 1;
    let dt = cfg::DT;

    // --- players: move, cooldowns, regen, region changes, deaths ---
    let mut deaths = Vec::new();
    let mut entered: Vec<(ClientId, cfg::Region)> = Vec::new();
    for (id, p) in world.players.iter_mut() {
        let speed = cfg::player_speed(p.gear);
        let (dx, dy) = (p.move_input[0], p.move_input[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len > 1.0 {
            p.pos = cfg::clamp_to_field(
                p.pos[0] + dx / len * speed * dt,
                p.pos[1] + dy / len * speed * dt,
            );
        } else {
            p.pos = cfg::clamp_to_field(
                p.pos[0] + dx * speed * dt,
                p.pos[1] + dy * speed * dt,
            );
        }

        p.attack_cd = (p.attack_cd - dt).max(0.0);
        // The sanctuary mends wounds much faster than the open field.
        let regen_rate = if cfg::in_safe_zone(p.pos[0], p.pos[1]) {
            cfg::SAFE_REGEN_PER_SEC
        } else {
            cfg::HP_REGEN_PER_SEC
        };
        p.regen_acc += dt * regen_rate;
        let max_hp = cfg::max_hp(p.gear);
        while p.regen_acc >= 1.0 {
            p.hp = (p.hp + 1).min(max_hp);
            p.regen_acc -= 1.0;
        }

        // Announce region transitions (both regions are `Copy` plain data).
        let ri = cfg::region_index(p.pos[0], p.pos[1]);
        if ri != p.region as usize {
            p.region = ri as u8;
            entered.push((*id, cfg::REGIONS[ri]));
        }

        if p.hp <= 0 {
            deaths.push(*id);
        }
    }
    for (id, region) in entered {
        world.logs.push(format!(
            "{} entered the {}.",
            hero_name(id),
            region.name
        ));
    }
    for id in deaths {
        if let Some(p) = world.players.get_mut(&id) {
            p.hp = cfg::max_hp(p.gear);
            p.regen_acc = 0.0;
            p.pos = cfg::clamp_to_field(cfg::SPAWN_POINT[0], cfg::SPAWN_POINT[1]);
        }
        world.logs.push(format!(
            "{} was flattened by a monster... embarrassing for an OP protagonist. \
             Reincarnated at the shrine.",
            hero_name(id)
        ));
    }

    // --- enemies: chase nearest player outside the sanctuary, attack ---
    for i in 0..world.enemies.len() {
        let (kind, epos) = {
            let e = &world.enemies[i];
            (e.kind, e.pos)
        };

        // Nearest player within chase range (the sanctuary is off-limits).
        let mut target: Option<[f32; 2]> = None;
        let mut best = cfg::ENEMY_CHASE_RANGE * cfg::ENEMY_CHASE_RANGE;
        for p in world.players.values() {
            if cfg::in_safe_zone(p.pos[0], p.pos[1]) {
                continue;
            }
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
                            if cfg::in_safe_zone(p.pos[0], p.pos[1]) {
                                return false;
                            }
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
                        let raw = cfg::enemy_damage(kind);
                        nearest.hp -= cfg::damage_taken(nearest.gear, raw);
                    }
                }
            }
        }
        // hit cooldown tick
        let enemy = &mut world.enemies[i];
        enemy.hit_cd = (enemy.hit_cd - dt).max(0.0);
    }

    // --- sanctuary keeps monsters out entirely: project them back outside ---
    {
        let (cx, cy) = (cfg::SAFE_ZONE_CENTER[0], cfg::SAFE_ZONE_CENTER[1]);
        let r = cfg::SAFE_ZONE_RADIUS + 4.0;
        for e in world.enemies.iter_mut() {
            let dx = e.pos[0] - cx;
            let dy = e.pos[1] - cy;
            let d2 = dx * dx + dy * dy;
            if d2 < r * r {
                let d = d2.sqrt();
                if d < 0.001 {
                    e.pos = [cx - r, cy];
                } else {
                    e.pos = [cx + dx / d * r, cy + dy / d * r];
                }
            }
        }
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

    // --- spawner: monsters appear inside wild regions (never the sanctuary
    //     or the homestead), with the region deciding the enemy mix ---
    world.spawn_timer -= dt;
    if world.spawn_timer <= 0.0 {
        world.spawn_timer = cfg::ENEMY_SPAWN_INTERVAL;
        if world.enemies.len() < cfg::ENEMY_CAP {
            let wilds = cfg::wild_regions();
            if !wilds.is_empty() {
                let mut rng = thread_rng();
                let region = wilds[rng.gen_range(0..wilds.len())];
                let kind = region.spawns[rng.gen_range(0..region.spawns.len())];
                // Spawn well inside the region, away from its borders.
                let pos = [
                    rng.gen_range((region.rect[0] + 40.0)..(region.rect[2] - 40.0)),
                    rng.gen_range((region.rect[1] + 40.0)..(region.rect[3] - 40.0)),
                ];
                if !cfg::in_safe_zone(pos[0], pos[1]) {
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
        }
    }

    // --- crops ---
    for plot in world.plots.iter_mut() {
        if let Some(crop) = plot.crop.as_mut() {
            let mult = if crop.watered { 1.0 } else { cfg::CROP_DRY_MULT };
            crop.growth = (crop.growth + dt / cfg::CROP_GROW_TIME * mult).min(1.0);
        }
    }

    // --- depleted resource nodes regrow over time ---
    for node in world.nodes.iter_mut() {
        if !node.ready {
            node.timer -= dt;
            if node.timer <= 0.0 {
                node.ready = true;
            }
        }
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
                wood: p.wood,
                stone: p.stone,
                ore: p.ore,
                turnips: p.turnips,
                potions: p.potions,
                gear: p.gear,
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
        nodes: world
            .nodes
            .iter()
            .map(|n| NodeSnap { ready: n.ready })
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
