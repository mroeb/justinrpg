//! Game client: rendering, input, client-side prediction for your own hero,
//! interpolation for everything else.
//!
//! Run the server first: `cargo run --bin server`, then `cargo run`.
//! Open several clients to see multiplayer in action.

use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::SystemTime;

use bevy::prelude::*;
use bevy::text::FontSize;
use bevy_renet::netcode::{
    ClientAuthentication, NetcodeClientPlugin, NetcodeClientTransport, NetcodeErrorEvent,
};
use bevy_renet::renet::{ConnectionConfig, DefaultChannel};
use bevy_renet::{client_connected, RenetClient, RenetClientPlugin};

use justin_rpg::config as cfg;
use justin_rpg::protocol::{ActionKind, C2S, PlotSnap, S2C};

fn main() {
    let (client_id, client, transport) = new_renet_client();

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "JustinRPG - Reincarnated as an OP Farmer".to_string(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(RenetClientPlugin)
        .add_plugins(NetcodeClientPlugin)
        .insert_resource(client)
        .insert_resource(transport)
        .insert_resource(NetId(client_id))
        // Prediction starts at the same place the server spawns us.
        .insert_resource(Prediction {
            pos: Vec2::from(cfg::SPAWN_POINT),
            facing: Vec2::Y,
        })
        .init_resource::<CurrentDir>()
        .init_resource::<LocalState>()
        .init_resource::<ServerOwnPos>()
        .init_resource::<RemotePlayers>()
        .init_resource::<EnemyEntities>()
        .init_resource::<PlotEntities>()
        .init_resource::<GameLog>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                read_input.run_if(client_connected),
                send_move.run_if(client_connected),
                receive.run_if(client_connected),
                predict,
                smooth_others,
                fade_flashes,
                camera_follow,
                update_hud,
            )
                .chain(),
        )
        .add_observer(on_netcode_error)
        .run();
}

fn new_renet_client() -> (u64, RenetClient, NetcodeClientTransport) {
    let server_addr = cfg::SERVER_ADDR.parse().unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    // Pid keeps ids unique when several clients start in the same millisecond.
    let client_id = ((u64::from(std::process::id()) << 32)
        | (current_time.as_millis() as u64 & 0xFFFF_FFFF))
        & 0x7FFF_FFFF_FFFF_FFFF;
    let authentication = ClientAuthentication::Unsecure {
        client_id,
        protocol_id: cfg::PROTOCOL_ID,
        server_addr,
        user_data: None,
    };
    let transport = NetcodeClientTransport::new(current_time, authentication, socket).unwrap();
    let client = RenetClient::new(ConnectionConfig::default());
    (client_id, client, transport)
}

// ---------------------------------------------------------------------------
// Resources & components
// ---------------------------------------------------------------------------

/// Our netcode client id (== our player id in snapshots).
#[derive(Resource)]
struct NetId(u64);

/// Locally known stats of our own hero (refreshed from snapshots).
#[derive(Resource, Default)]
struct LocalState {
    hp: i32,
    seeds: i32,
    harvests: i32,
    kills: u32,
}

/// Client-side prediction of our own position (+ last facing for attack VFX).
#[derive(Resource)]
struct Prediction {
    pos: Vec2,
    facing: Vec2,
}

/// Current normalized movement direction (from keyboard).
#[derive(Resource, Default)]
struct CurrentDir(Vec2);

/// Latest authoritative position of our hero; used to correct prediction.
#[derive(Resource, Default)]
struct ServerOwnPos(Option<Vec2>);

/// server player id -> rendered entity (remote players only).
#[derive(Resource, Default)]
struct RemotePlayers(HashMap<u64, Entity>);

/// server enemy id -> rendered entity.
#[derive(Resource, Default)]
struct EnemyEntities(HashMap<u32, Entity>);

/// Plot entities, row-major (index == `config::plot_pos` index).
#[derive(Resource, Default)]
struct PlotEntities(Vec<Entity>);

/// Last few server log lines (bottom-left chat box).
#[derive(Resource, Default)]
struct GameLog(Vec<String>);

#[derive(Component)]
struct LocalHero;

/// Destination for exponential-smoothed network interpolation.
#[derive(Component)]
struct RemoteTarget(Vec2);

/// Short-lived attack swing visual.
#[derive(Component)]
struct Flash {
    life: f32,
    max: f32,
}

#[derive(Component)]
struct HudText;

#[derive(Component)]
struct LogText;

#[derive(Component)]
struct MainCamera;

// ---------------------------------------------------------------------------
// Visual helpers (no assets in this project: colored sprites + default font)
// ---------------------------------------------------------------------------

fn plot_color(p: &PlotSnap) -> Color {
    match &p.crop {
        None if !p.tilled => Color::srgb(0.55, 0.47, 0.30), // untilled marker
        None => Color::srgb(0.34, 0.22, 0.12),              // tilled soil
        Some(c) if c.growth >= 1.0 => Color::srgb(0.98, 0.85, 0.20), // ripe & golden
        Some(c) => {
            let t = c.growth.clamp(0.0, 1.0);
            let mut col = Color::srgb(0.15 + 0.35 * t, 0.45 + 0.45 * t, 0.15);
            if !c.watered {
                col = col.darker(0.45); // dry crops look thirsty: press E to water
            }
            col
        }
    }
}

fn enemy_color(kind: u8) -> Color {
    if kind == 0 {
        Color::srgb(0.30, 0.75, 0.30) // slime
    } else {
        Color::srgb(0.35, 0.30, 0.48) // shadow wolf
    }
}

fn enemy_size(kind: u8) -> Vec2 {
    if kind == 0 {
        Vec2::splat(40.0)
    } else {
        Vec2::splat(48.0)
    }
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

fn setup(
    mut commands: Commands,
    prediction: Res<Prediction>,
    mut plot_entities: ResMut<PlotEntities>,
) {
    commands.spawn((
        Camera2d,
        Transform::from_xyz(prediction.pos.x, prediction.pos.y, 100.0),
        MainCamera,
    ));

    // Ground field.
    commands.spawn((
        Sprite::from_color(
            Color::srgb(0.23, 0.45, 0.20),
            Vec2::new(cfg::FIELD_HALF[0] * 2.0, cfg::FIELD_HALF[1] * 2.0),
        ),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    // Farm plots (colors are updated from snapshots).
    for i in 0..cfg::plot_count() {
        let p = cfg::plot_pos(i);
        let entity = commands
            .spawn((
                Sprite::from_color(Color::srgb(0.55, 0.47, 0.30), Vec2::splat(cfg::PLOT_SIZE)),
                Transform::from_xyz(p[0], p[1], 1.0),
            ))
            .id();
        plot_entities.0.push(entity);
    }

    // Our hero (gold = the protagonist).
    commands.spawn((
        Sprite::from_color(Color::srgb(1.0, 0.85, 0.15), Vec2::splat(46.0)),
        Transform::from_xyz(prediction.pos.x, prediction.pos.y, 3.0),
        LocalHero,
    ));

    // HUD: stats (top-left).
    commands.spawn((
        Text::new("HP -"),
        TextFont {
            font_size: FontSize::Px(22.0),
            ..default()
        },
        TextColor(Color::srgb(1.0, 1.0, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            top: px(8.0),
            left: px(10.0),
            ..default()
        },
        HudText,
    ));

    // HUD: controls (top-right).
    commands.spawn((
        Text::new("WASD move  SPACE one-punch  E till/plant/water/harvest"),
        TextFont {
            font_size: FontSize::Px(16.0),
            ..default()
        },
        TextColor(Color::srgba(1.0, 1.0, 1.0, 0.7)),
        Node {
            position_type: PositionType::Absolute,
            top: px(8.0),
            right: px(10.0),
            ..default()
        },
    ));

    // HUD: server chat/log (bottom-left).
    commands.spawn((
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(16.0),
            ..default()
        },
        TextColor(Color::srgb(0.95, 0.9, 0.6)),
        Node {
            position_type: PositionType::Absolute,
            bottom: px(8.0),
            left: px(10.0),
            max_width: px(700.0),
            ..default()
        },
        LogText,
    ));

    commands.spawn((
        Text::new("Connecting to the other world..."),
        TextFont {
            font_size: FontSize::Px(28.0),
            ..default()
        },
        TextColor(Color::srgb(1.0, 0.6, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            bottom: px(60.0),
            left: px(10.0),
            ..default()
        },
    ));
}

// ---------------------------------------------------------------------------
// Input & networking
// ---------------------------------------------------------------------------

fn read_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut dir: ResMut<CurrentDir>,
    prediction: Res<Prediction>,
    mut commands: Commands,
    mut client: ResMut<RenetClient>,
) {
    let mut v = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        v.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        v.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        v.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        v.x += 1.0;
    }
    if v.length_squared() > 1.0 {
        v = v.normalize();
    }
    dir.0 = v;

    if keys.just_pressed(KeyCode::Space) {
        let msg = C2S::Action(ActionKind::Attack);
        client.send_message(
            DefaultChannel::ReliableOrdered,
            bincode::serialize(&msg).unwrap(),
        );
        // Local swing visual (cosmetic; the server decides actual hits).
        let facing = if prediction.facing.length_squared() < 0.01 {
            Vec2::Y
        } else {
            prediction.facing
        };
        let center = prediction.pos + facing * 48.0;
        commands.spawn((
            Sprite::from_color(Color::srgb(1.0, 0.95, 0.6), Vec2::splat(56.0)),
            Transform::from_xyz(center.x, center.y, 4.0),
            Flash {
                life: 0.15,
                max: 0.15,
            },
        ));
    }
    if keys.just_pressed(KeyCode::KeyE) {
        let msg = C2S::Action(ActionKind::Farm);
        client.send_message(
            DefaultChannel::ReliableOrdered,
            bincode::serialize(&msg).unwrap(),
        );
    }
}

fn send_move(dir: Res<CurrentDir>, mut client: ResMut<RenetClient>) {
    let msg = C2S::Move {
        dir: dir.0.to_array(),
    };
    client.send_message(
        DefaultChannel::Unreliable,
        bincode::serialize(&msg).unwrap(),
    );
}

fn receive(
    mut client: ResMut<RenetClient>,
    mut commands: Commands,
    net_id: Res<NetId>,
    mut local: ResMut<LocalState>,
    mut server_own: ResMut<ServerOwnPos>,
    mut remotes: ResMut<RemotePlayers>,
    mut enemy_entities: ResMut<EnemyEntities>,
    plot_entities: Res<PlotEntities>,
    mut game_log: ResMut<GameLog>,
    mut sprites: Query<&mut Sprite>,
) {
    while let Some(bytes) = client.receive_message(DefaultChannel::Unreliable) {
        let Ok(S2C::Snapshot {
            players,
            enemies,
            plots,
            ..
        }) = bincode::deserialize(&bytes)
        else {
            continue;
        };

        // Own stats + authoritative position (for prediction correction).
        for p in players.iter().filter(|p| p.id == net_id.0) {
            local.hp = p.hp;
            local.seeds = p.seeds;
            local.harvests = p.harvests;
            local.kills = p.kills;
            server_own.0 = Some(Vec2::from(p.pos));
        }

        // Remote players: spawn / update / despawn.
        for p in players.iter().filter(|p| p.id != net_id.0) {
            let target = Vec2::from(p.pos);
            match remotes.0.get(&p.id) {
                Some(&entity) => {
                    commands.entity(entity).insert(RemoteTarget(target));
                }
                None => {
                    let entity = commands
                        .spawn((
                            Sprite::from_color(Color::srgb(0.92, 0.55, 0.25), Vec2::splat(46.0)),
                            Transform::from_xyz(target.x, target.y, 3.0),
                            RemoteTarget(target),
                        ))
                        .id();
                    remotes.0.insert(p.id, entity);
                    push_log(
                        &mut game_log,
                        format!("Hero-{:04} entered the world.", p.id % 10000),
                    );
                }
            }
        }
        let present: Vec<u64> = players.iter().map(|p| p.id).collect();
        remotes.0.retain(|id, entity| {
            if present.contains(id) {
                true
            } else {
                commands.entity(*entity).despawn();
                push_log(&mut game_log, format!("Hero-{id:04} left the world."));
                false
            }
        });

        // Enemies: spawn / update / despawn.
        for e in &enemies {
            let target = Vec2::from(e.pos);
            match enemy_entities.0.get(&e.id) {
                Some(&entity) => {
                    commands.entity(entity).insert(RemoteTarget(target));
                }
                None => {
                    let entity = commands
                        .spawn((
                            Sprite::from_color(enemy_color(e.kind), enemy_size(e.kind)),
                            Transform::from_xyz(target.x, target.y, 2.0),
                            RemoteTarget(target),
                        ))
                        .id();
                    enemy_entities.0.insert(e.id, entity);
                }
            }
        }
        let present_enemies: Vec<u32> = enemies.iter().map(|e| e.id).collect();
        enemy_entities.0.retain(|id, entity| {
            if present_enemies.contains(id) {
                true
            } else {
                commands.entity(*entity).despawn();
                false
            }
        });

        // Plot colors (row-major index match).
        for (i, plot) in plots.iter().enumerate() {
            if let Some(&entity) = plot_entities.0.get(i) {
                if let Ok(mut sprite) = sprites.get_mut(entity) {
                    sprite.color = plot_color(plot);
                }
            }
        }
    }

    while let Some(bytes) = client.receive_message(DefaultChannel::ReliableOrdered) {
        if let Ok(S2C::Log { text }) = bincode::deserialize(&bytes) {
            push_log(&mut game_log, text);
        }
    }
}

fn push_log(log: &mut GameLog, line: String) {
    const MAX_LINES: usize = 6;
    log.0.push(line);
    if log.0.len() > MAX_LINES {
        log.0.remove(0);
    }
}

// ---------------------------------------------------------------------------
// Simulation/presentation systems
// ---------------------------------------------------------------------------

/// Client-side prediction for our own hero + reconciliation with the server.
fn predict(
    time: Res<Time>,
    dir: Res<CurrentDir>,
    server_own: Res<ServerOwnPos>,
    mut prediction: ResMut<Prediction>,
    mut hero: Query<&mut Transform, With<LocalHero>>,
) {
    let dt = time.delta().as_secs_f32().min(0.1);
    let mut p = prediction.pos + dir.0 * cfg::PLAYER_SPEED * dt;
    p = Vec2::from(cfg::clamp_to_field(p.x, p.y));

    if let Some(server_pos) = server_own.0 {
        let d = p.distance(server_pos);
        if d > 150.0 {
            // Far off (death respawn, teleport): hard snap.
            p = server_pos;
        } else if d > 30.0 {
            // Small desync from latency: ease toward the authoritative position.
            p = p.lerp(server_pos, 1.0 - (-6.0 * dt).exp());
        }
    }
    prediction.pos = p;
    if dir.0.length_squared() > 0.01 {
        prediction.facing = dir.0.normalize();
    }

    if let Ok(mut t) = hero.single_mut() {
        t.translation.x = p.x;
        t.translation.y = p.y;
    }
}

/// Interpolate remote players and enemies toward their last snapshot position.
fn smooth_others(time: Res<Time>, mut query: Query<(&mut Transform, &RemoteTarget)>) {
    let dt = time.delta().as_secs_f32().min(0.1);
    let k = 1.0 - (-15.0 * dt).exp();
    for (mut transform, target) in &mut query {
        transform.translation.x += (target.0.x - transform.translation.x) * k;
        transform.translation.y += (target.0.y - transform.translation.y) * k;
    }
}

fn fade_flashes(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut Flash, &mut Sprite)>,
) {
    let dt = time.delta().as_secs_f32();
    for (entity, mut flash, mut sprite) in &mut query {
        flash.life -= dt;
        if flash.life <= 0.0 {
            commands.entity(entity).despawn();
        } else {
            let a = 0.9 * flash.life / flash.max;
            sprite.color = sprite.color.with_alpha(a);
        }
    }
}

fn camera_follow(
    time: Res<Time>,
    prediction: Res<Prediction>,
    mut camera: Query<&mut Transform, With<MainCamera>>,
) {
    let dt = time.delta().as_secs_f32().min(0.1);
    let k = 1.0 - (-10.0 * dt).exp();
    if let Ok(mut t) = camera.single_mut() {
        t.translation.x += (prediction.pos.x - t.translation.x) * k;
        t.translation.y += (prediction.pos.y - t.translation.y) * k;
        t.translation.z = 100.0;
    }
}

fn update_hud(
    local: Res<LocalState>,
    game_log: Res<GameLog>,
    mut hud: Query<&mut Text, (With<HudText>, Without<LogText>)>,
    mut log_view: Query<&mut Text, With<LogText>>,
) {
    if let Ok(mut text) = hud.single_mut() {
        **text = format!(
            "HP {}/{}   Seeds {}   Harvests {}   Kills {}",
            local.hp.max(0),
            cfg::PLAYER_MAX_HP,
            local.seeds,
            local.harvests,
            local.kills,
        );
    }
    if let Ok(mut text) = log_view.single_mut() {
        **text = game_log.0.join("\n");
    }
}

// ---------------------------------------------------------------------------

fn on_netcode_error(error: On<NetcodeErrorEvent>) {
    panic!(
        "Network error: {}\n\nIs the server running? Start it with: cargo run --bin server",
        *error
    );
}
