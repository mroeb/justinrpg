//! Game client: rendering, input, client-side prediction for your own hero,
//! interpolation for everything else.
//!
//! Run the server first: `cargo run --bin server`, then `cargo run`.
//! Open several clients to see multiplayer in action.

use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::SystemTime;

use bevy::prelude::*;
use bevy::text::{FontSize, Justify, TextLayout};
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
        .init_resource::<NodeEntities>()
        .init_resource::<NodeStates>()
        .init_resource::<GameLog>()
        .init_resource::<RegionNow>()
        .init_resource::<BannerTimer>()
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
                pulse_shrine,
                camera_follow,
                region_watch,
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
#[derive(Resource)]
struct LocalState {
    hp: i32,
    seeds: i32,
    harvests: i32,
    kills: u32,
    wood: i32,
    stone: i32,
    ore: i32,
    turnips: i32,
    potions: i32,
    /// Equipment bitmask (`cfg::GEAR_*`); drives predicted speed and the HUD.
    gear: u8,
}

impl Default for LocalState {
    fn default() -> Self {
        Self {
            hp: cfg::PLAYER_MAX_HP,
            seeds: 0,
            harvests: 0,
            kills: 0,
            wood: 0,
            stone: 0,
            ore: 0,
            turnips: 0,
            potions: 0,
            gear: 0,
        }
    }
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

/// Resource-node sprite entities, index == `config::node_pos`.
#[derive(Resource, Default)]
struct NodeEntities(Vec<Entity>);

/// Last-known readiness of every resource node (index == `config::node_pos`).
#[derive(Resource, Default)]
struct NodeStates(Vec<bool>);

/// Last few server log lines (bottom-left chat box).
#[derive(Resource, Default)]
struct GameLog(Vec<String>);

/// Name of the region we were in last frame (drives the entry banner).
#[derive(Resource, Default)]
struct RegionNow(String);

/// Seconds left on the region-entry banner's fade-out.
#[derive(Resource, Default)]
struct BannerTimer(f32);

/// Entity handles for every mutable HUD element, so the HUD systems can use
/// one unfiltered `Query<&mut Text>` instead of a pile of conflicting filters.
#[derive(Resource)]
struct UiHandles {
    hp_label: Entity,
    hp_fill: Entity,
    stats: Entity,
    inv: Vec<Entity>,
    craft: Vec<Entity>,
    hint: Entity,
    log: Entity,
    region: Entity,
    banner: Entity,
}

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

/// Pulsing golden glow of the sanctuary floor (base alpha per instance).
#[derive(Component)]
struct SanctuaryGlow {
    base: f32,
}

#[derive(Component)]
struct MainCamera;

// ---------------------------------------------------------------------------
// Visual helpers (no assets in this project: colored sprites + default font)
// ---------------------------------------------------------------------------

const INV_LABELS: [&str; 5] = ["Wood", "Stone", "Ore", "Turnips", "Draughts"];

fn inv_color(i: usize) -> Color {
    match i {
        0 => Color::srgb(0.75, 0.55, 0.30),
        1 => Color::srgb(0.78, 0.78, 0.82),
        2 => Color::srgb(0.78, 0.55, 0.95),
        3 => Color::srgb(0.95, 0.85, 0.35),
        _ => Color::srgb(0.45, 0.95, 0.55),
    }
}

/// Ground tint per region (index matches `config::REGIONS`).
fn region_color(i: usize) -> Color {
    match i {
        0 => Color::srgba(0.98, 0.90, 0.60, 0.35), // sanctuary sand
        1 => Color::srgba(0.08, 0.30, 0.12, 0.55), // whisperwood
        2 => Color::srgba(0.48, 0.22, 0.09, 0.55), // emberveins
        3 => Color::srgba(0.45, 0.42, 0.20, 0.45), // homestead
        4 => Color::srgba(0.40, 0.40, 0.44, 0.60), // quarry
        5 => Color::srgba(0.26, 0.18, 0.40, 0.60), // gloamfens
        _ => Color::srgba(0.26, 0.48, 0.22, 0.45), // western meadow
    }
}

fn node_color(kind: u8, ready: bool) -> Color {
    let base = match kind {
        0 => Color::srgb(0.10, 0.42, 0.14), // timber tree
        1 => Color::srgb(0.52, 0.52, 0.56), // boulder
        _ => Color::srgb(0.55, 0.32, 0.85), // ore vein
    };
    if ready {
        base
    } else {
        base.darker(0.65) // depleted nodes look withered until they regrow
    }
}

fn node_size(kind: u8) -> Vec2 {
    match kind {
        0 => Vec2::new(44.0, 54.0),
        1 => Vec2::splat(44.0),
        _ => Vec2::splat(38.0),
    }
}

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
    match kind {
        0 => Color::srgb(0.30, 0.75, 0.30), // slime
        1 => Color::srgb(0.35, 0.30, 0.48), // shadow wolf
        _ => Color::srgb(0.90, 0.45, 0.15), // ember golem
    }
}

fn enemy_size(kind: u8) -> Vec2 {
    match kind {
        0 => Vec2::splat(40.0),
        1 => Vec2::splat(48.0),
        _ => Vec2::splat(64.0),
    }
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

fn setup(
    mut commands: Commands,
    prediction: Res<Prediction>,
    mut plot_entities: ResMut<PlotEntities>,
    mut node_entities: ResMut<NodeEntities>,
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

    // Region ground overlays + map-style name labels (labels ride above the
    // world on purpose, like a zone title card).
    for (i, region) in cfg::REGIONS.iter().enumerate() {
        let w = region.rect[2] - region.rect[0];
        let h = region.rect[3] - region.rect[1];
        let cx = (region.rect[0] + region.rect[2]) * 0.5;
        let cy = (region.rect[1] + region.rect[3]) * 0.5;
        commands.spawn((
            Sprite::from_color(region_color(i), Vec2::new(w, h)),
            Transform::from_xyz(cx, cy, 0.5),
        ));
        // The sanctuary gets its own dedicated label below the safe zone.
        if i != 0 {
            commands.spawn((
                Text2d::new(region.name.to_string()),
                TextFont {
                    font_size: FontSize::Px(40.0),
                    ..default()
                },
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.30)),
                TextLayout::justify(Justify::Center),
                Transform::from_xyz(cx, cy, 6.0),
            ));
        }
    }

    // Sanctuary safe zone: golden diamond floor + label (server enforces it).
    let (sx, sy) = (cfg::SAFE_ZONE_CENTER[0], cfg::SAFE_ZONE_CENTER[1]);
    let side = cfg::SAFE_ZONE_RADIUS * std::f32::consts::SQRT_2;
    let diamond = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);
    commands.spawn((
        Sprite::from_color(Color::srgba(1.0, 0.92, 0.5, 1.0), Vec2::splat(side)),
        Transform::from_xyz(sx, sy, 0.6).with_rotation(diamond),
        SanctuaryGlow { base: 0.20 },
    ));
    commands.spawn((
        Sprite::from_color(Color::srgba(1.0, 0.96, 0.75, 1.0), Vec2::splat(side * 0.72)),
        Transform::from_xyz(sx, sy, 0.6).with_rotation(diamond),
        SanctuaryGlow { base: 0.10 },
    ));
    commands.spawn((
        Text2d::new("SANCTUARY - SAFE ZONE"),
        TextFont {
            font_size: FontSize::Px(26.0),
            ..default()
        },
        TextColor(Color::srgba(1.0, 0.95, 0.7, 0.8)),
        TextLayout::justify(Justify::Center),
        Transform::from_xyz(sx, sy - cfg::SAFE_ZONE_RADIUS - 34.0, 6.0),
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

    // Resource nodes (ready/depleted colors come from snapshots).
    for i in 0..cfg::node_count() {
        let p = cfg::node_pos(i);
        let kind = cfg::node_kind(i);
        let entity = commands
            .spawn((
                Sprite::from_color(node_color(kind, true), node_size(kind)),
                Transform::from_xyz(p[0], p[1], 1.5),
            ))
            .id();
        node_entities.0.push(entity);
        commands.spawn((
            Text2d::new(cfg::NODE_NAMES[kind as usize].to_string()),
            TextFont {
                font_size: FontSize::Px(14.0),
                ..default()
            },
            TextColor(Color::srgba(1.0, 1.0, 1.0, 0.45)),
            TextLayout::justify(Justify::Center),
            Transform::from_xyz(p[0], p[1] - 38.0, 1.5),
        ));
    }

    // Our hero (gold = the protagonist).
    commands.spawn((
        Sprite::from_color(Color::srgb(1.0, 0.85, 0.15), Vec2::splat(46.0)),
        Transform::from_xyz(prediction.pos.x, prediction.pos.y, 3.0),
        LocalHero,
    ));

    // --- HUD: stats panel (top-left): title, HP label, HP bar, stat lines ---
    let mut hp_label = Entity::PLACEHOLDER;
    let mut hp_fill = Entity::PLACEHOLDER;
    let mut stats = Entity::PLACEHOLDER;
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(10.0),
                left: px(10.0),
                width: px(300.0),
                padding: UiRect::all(px(12.0)),
                flex_direction: FlexDirection::Column,
                row_gap: px(6.0),
                border: UiRect::all(px(2.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.05, 0.10, 0.82)),
            BorderColor::all(Color::srgba(0.85, 0.75, 0.35, 0.9)),
        ))
        .with_children(|panel| {
            hp_label = panel
                .spawn((
                    Text::new("HP 100/100"),
                    TextFont {
                        font_size: FontSize::Px(20.0),
                        ..default()
                    },
                    TextColor(Color::srgb(1.0, 1.0, 1.0)),
                ))
                .id();
            let mut fill = Entity::PLACEHOLDER;
            panel
                .spawn((
                    Node {
                        width: px(272.0),
                        height: px(18.0),
                        border: UiRect::all(px(1.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.25, 0.06, 0.06, 1.0)),
                    BorderColor::all(Color::srgba(0.0, 0.0, 0.0, 0.7)),
                ))
                .with_children(|bar| {
                    fill = bar
                        .spawn((
                            Node {
                                width: percent(100.0),
                                height: percent(100.0),
                                ..default()
                            },
                            BackgroundColor(Color::srgb(0.25, 0.85, 0.30)),
                        ))
                        .id();
                });
            hp_fill = fill;
            stats = panel
                .spawn((
                    Text::new("Kills 0   Harvests 0   Seeds 0\nGear: nothing crafted yet"),
                    TextFont {
                        font_size: FontSize::Px(15.0),
                        ..default()
                    },
                    TextColor(Color::srgba(0.9, 0.92, 1.0, 0.9)),
                ))
                .id();
        });

    // --- HUD: inventory panel (top-right): resources + control reference ---
    let mut inv: Vec<Entity> = Vec::new();
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: px(10.0),
                right: px(10.0),
                width: px(230.0),
                padding: UiRect::all(px(12.0)),
                flex_direction: FlexDirection::Column,
                row_gap: px(4.0),
                border: UiRect::all(px(2.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.05, 0.10, 0.82)),
            BorderColor::all(Color::srgba(0.45, 0.65, 0.90, 0.9)),
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new("INVENTORY"),
                TextFont {
                    font_size: FontSize::Px(16.0),
                    ..default()
                },
                TextColor(Color::srgb(1.0, 0.85, 0.35)),
            ));
            for (i, label) in INV_LABELS.iter().enumerate() {
                inv.push(
                    panel
                        .spawn((
                            Text::new(format!("{label:<10} {:>4}", 0)),
                            TextFont {
                                font_size: FontSize::Px(15.0),
                                ..default()
                            },
                            TextColor(inv_color(i)),
                        ))
                        .id(),
                );
            }
            panel.spawn((
                Text::new(
                    "WASD move   SPACE punch\nE farm   G gather   Q drink\n1-6 craft at the workbench",
                ),
                TextFont {
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(Color::srgba(1.0, 1.0, 1.0, 0.65)),
            ));
        });

    // --- HUD: workbench panel (bottom-right): every recipe + hotkey ---
    let mut craft: Vec<Entity> = Vec::new();
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: px(10.0),
                right: px(10.0),
                width: px(470.0),
                padding: UiRect::all(px(12.0)),
                flex_direction: FlexDirection::Column,
                row_gap: px(2.0),
                border: UiRect::all(px(2.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.05, 0.10, 0.82)),
            BorderColor::all(Color::srgba(0.85, 0.55, 0.30, 0.9)),
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new("WORKBENCH - press 1-6 to craft"),
                TextFont {
                    font_size: FontSize::Px(15.0),
                    ..default()
                },
                TextColor(Color::srgb(1.0, 0.85, 0.35)),
            ));
            for _ in 0..cfg::RECIPES.len() {
                craft.push(
                    panel
                        .spawn((
                            Text::new(""),
                            TextFont {
                                font_size: FontSize::Px(14.0),
                                ..default()
                            },
                            TextColor(Color::srgb(1.0, 1.0, 1.0)),
                        ))
                        .id(),
                );
            }
        });

    // --- HUD: server chat/log (bottom-left) ---
    let mut log = Entity::PLACEHOLDER;
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                bottom: px(10.0),
                left: px(10.0),
                padding: UiRect::all(px(10.0)),
                border: UiRect::all(px(2.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.05, 0.10, 0.72)),
            BorderColor::all(Color::srgba(0.55, 0.55, 0.65, 0.6)),
        ))
        .with_children(|panel| {
            log = panel
                .spawn((
                    Text::new(""),
                    TextFont {
                        font_size: FontSize::Px(15.0),
                        ..default()
                    },
                    TextColor(Color::srgb(0.95, 0.9, 0.6)),
                    Node {
                        width: px(620.0),
                        ..default()
                    },
                ))
                .id();
        });

    // --- HUD: current region (top-center) + transient entry banner ---
    let region = commands
        .spawn((
            Text::new("Region: -"),
            TextFont {
                font_size: FontSize::Px(15.0),
                ..default()
            },
            TextColor(Color::srgba(1.0, 1.0, 1.0, 0.65)),
            TextLayout::justify(Justify::Center),
            Node {
                position_type: PositionType::Absolute,
                top: px(40.0),
                left: px(0.0),
                right: px(0.0),
                ..default()
            },
        ))
        .id();
    let banner = commands
        .spawn((
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(30.0),
                ..default()
            },
            TextColor(Color::srgba(1.0, 0.92, 0.55, 0.0)),
            TextLayout::justify(Justify::Center),
            Node {
                position_type: PositionType::Absolute,
                top: px(66.0),
                left: px(0.0),
                right: px(0.0),
                ..default()
            },
        ))
        .id();

    // --- HUD: contextual action hint (bottom-center) ---
    let hint = commands
        .spawn((
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(17.0),
                ..default()
            },
            TextColor(Color::srgba(1.0, 1.0, 1.0, 0.9)),
            TextLayout::justify(Justify::Center),
            Node {
                position_type: PositionType::Absolute,
                bottom: px(150.0),
                left: px(0.0),
                right: px(0.0),
                ..default()
            },
        ))
        .id();

    commands.insert_resource(UiHandles {
        hp_label,
        hp_fill,
        stats,
        inv,
        craft,
        hint,
        log,
        region,
        banner,
    });
}

// ---------------------------------------------------------------------------
// Input & networking
// ---------------------------------------------------------------------------

fn send_action(client: &mut RenetClient, kind: ActionKind) {
    client.send_message(
        DefaultChannel::ReliableOrdered,
        bincode::serialize(&C2S::Action(kind)).unwrap(),
    );
}

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
        send_action(&mut client, ActionKind::Attack);
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
        send_action(&mut client, ActionKind::Farm);
    }
    if keys.just_pressed(KeyCode::KeyG) {
        send_action(&mut client, ActionKind::Gather);
    }
    if keys.just_pressed(KeyCode::KeyQ) {
        send_action(&mut client, ActionKind::Drink);
    }
    const CRAFT_KEYS: [KeyCode; 6] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
    ];
    for (i, key) in CRAFT_KEYS.iter().enumerate() {
        if keys.just_pressed(*key) {
            send_action(&mut client, ActionKind::Craft(i as u8));
        }
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
    node_entities: Res<NodeEntities>,
    mut node_states: ResMut<NodeStates>,
    mut game_log: ResMut<GameLog>,
    mut sprites: Query<&mut Sprite>,
) {
    while let Some(bytes) = client.receive_message(DefaultChannel::Unreliable) {
        let Ok(S2C::Snapshot {
            players,
            enemies,
            plots,
            nodes,
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
            local.wood = p.wood;
            local.stone = p.stone;
            local.ore = p.ore;
            local.turnips = p.turnips;
            local.potions = p.potions;
            local.gear = p.gear;
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

        // Resource node readiness (index match), for sprites + context hints.
        node_states.0 = nodes.iter().map(|n| n.ready).collect();
        for (i, node) in nodes.iter().enumerate() {
            if let Some(&entity) = node_entities.0.get(i) {
                if let Ok(mut sprite) = sprites.get_mut(entity) {
                    sprite.color = node_color(cfg::node_kind(i), node.ready);
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
    const MAX_LINES: usize = 8;
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
    local: Res<LocalState>,
    server_own: Res<ServerOwnPos>,
    mut prediction: ResMut<Prediction>,
    mut hero: Query<&mut Transform, With<LocalHero>>,
) {
    let dt = time.delta().as_secs_f32().min(0.1);
    // Speed must match the server exactly, Wind Boots included.
    let speed = cfg::player_speed(local.gear);
    let mut p = prediction.pos + dir.0 * speed * dt;
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

/// Breathing golden glow on the sanctuary floor.
fn pulse_shrine(time: Res<Time>, mut query: Query<(&SanctuaryGlow, &mut Sprite)>) {
    let t = time.elapsed().as_secs_f32();
    let wave = 0.85 + 0.35 * (t * 1.5).sin();
    for (glow, mut sprite) in &mut query {
        sprite.color = sprite.color.with_alpha((glow.base * wave).clamp(0.0, 1.0));
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

/// Shows a fading banner whenever we cross into a new region.
fn region_watch(
    time: Res<Time>,
    prediction: Res<Prediction>,
    mut now: ResMut<RegionNow>,
    mut timer: ResMut<BannerTimer>,
    ui: Res<UiHandles>,
    mut texts: Query<&mut Text>,
    mut colors: Query<&mut TextColor>,
) {
    let region = cfg::region_at(prediction.pos.x, prediction.pos.y);
    if region.name != now.0 {
        now.0 = region.name.to_string();
        timer.0 = 3.5;
        if let Ok(mut text) = texts.get_mut(ui.banner) {
            **text = format!("{}\n{}", region.name, region.desc);
        }
        if let Ok(mut color) = colors.get_mut(ui.banner) {
            *color = TextColor(Color::srgba(1.0, 0.92, 0.55, 1.0));
        }
        if let Ok(mut text) = texts.get_mut(ui.region) {
            **text = format!("Region: {}", region.name);
        }
    }

    if timer.0 > 0.0 {
        timer.0 -= time.delta().as_secs_f32();
        if timer.0 <= 0.0 {
            timer.0 = 0.0;
            if let Ok(mut text) = texts.get_mut(ui.banner) {
                **text = String::new();
            }
        } else {
            // Hold full opacity, then fade over the last second.
            let alpha = timer.0.min(1.0);
            if let Ok(mut color) = colors.get_mut(ui.banner) {
                color.0 = color.0.with_alpha(alpha);
            }
        }
    }
}

/// What would E/G/Q do right now? Spelled out just above the chat box.
fn context_hint(local: &LocalState, nodes: &NodeStates, pos: Vec2) -> String {
    let mut parts: Vec<String> = Vec::new();
    if cfg::nearest_plot(pos.x, pos.y).is_some() {
        parts.push("[E] tend farm plot".to_string());
    }
    if let Some(ni) = cfg::nearest_node(pos.x, pos.y) {
        let kind = cfg::node_kind(ni) as usize;
        if nodes.0.get(ni).copied() == Some(false) {
            parts.push(format!(
                "[G] {} depleted - regrowing",
                cfg::NODE_NAMES[kind]
            ));
        } else {
            parts.push(format!(
                "[G] gather {} ({})",
                cfg::NODE_RESOURCE_NAMES[kind], cfg::NODE_NAMES[kind]
            ));
        }
    }
    if local.potions > 0 && local.hp < cfg::max_hp(local.gear) {
        parts.push(format!("[Q] drink draught (x{})", local.potions));
    }
    if parts.is_empty() {
        "WASD move    SPACE one-punch    [1-6] craft    gather wood, stone & ore out in the wilds"
            .to_string()
    } else {
        parts.join("     ")
    }
}

fn update_hud(
    local: Res<LocalState>,
    prediction: Res<Prediction>,
    node_states: Res<NodeStates>,
    game_log: Res<GameLog>,
    ui: Res<UiHandles>,
    mut texts: Query<&mut Text>,
    mut colors: Query<&mut TextColor>,
    mut ui_nodes: Query<&mut Node>,
    mut backgrounds: Query<&mut BackgroundColor>,
) {
    let max_hp = cfg::max_hp(local.gear);
    let hp = local.hp.max(0);
    let frac = (hp as f32 / max_hp as f32).clamp(0.0, 1.0);
    let safe = cfg::in_safe_zone(prediction.pos.x, prediction.pos.y);

    if let Ok(mut text) = texts.get_mut(ui.hp_label) {
        **text = if safe {
            format!("HP {hp}/{max_hp}   [SANCTUARY]")
        } else {
            format!("HP {hp}/{max_hp}")
        };
    }
    if let Ok(mut node) = ui_nodes.get_mut(ui.hp_fill) {
        node.width = percent(frac * 100.0);
    }
    if let Ok(mut bg) = backgrounds.get_mut(ui.hp_fill) {
        bg.0 = if frac > 0.5 {
            Color::srgb(0.25, 0.85, 0.30)
        } else if frac > 0.0 {
            Color::srgb(0.95, 0.75, 0.20)
        } else {
            Color::srgb(0.90, 0.25, 0.20)
        };
    }

    if let Ok(mut text) = texts.get_mut(ui.stats) {
        **text = format!(
            "Kills {}   Harvests {}   Seeds {}\nGear: {}",
            local.kills,
            local.harvests,
            local.seeds,
            cfg::gear_names(local.gear)
        );
    }

    let values = [
        local.wood,
        local.stone,
        local.ore,
        local.turnips,
        local.potions,
    ];
    for (i, &entity) in ui.inv.iter().enumerate() {
        if let Ok(mut text) = texts.get_mut(entity) {
            **text = format!("{:<10} {:>4}", INV_LABELS[i], values[i]);
        }
    }

    for (i, &entity) in ui.craft.iter().enumerate() {
        let Some(recipe) = cfg::RECIPES.get(i) else { continue };
        let owned = recipe.gear != 0 && (local.gear & recipe.gear != 0);
        let afford = cfg::can_afford(
            recipe,
            local.wood,
            local.stone,
            local.ore,
            local.turnips,
        );
        let status = if owned {
            "  [OWNED]"
        } else if afford {
            "  [READY]"
        } else {
            ""
        };
        if let Ok(mut text) = texts.get_mut(entity) {
            **text = format!(
                "[{}] {:<15} {:>7}  {}{}",
                i + 1,
                recipe.name,
                cfg::cost_text(recipe),
                recipe.blurb,
                status
            );
        }
        if let Ok(mut color) = colors.get_mut(entity) {
            *color = TextColor(if owned {
                Color::srgb(1.0, 0.85, 0.35)
            } else if afford {
                Color::srgb(0.45, 0.95, 0.50)
            } else {
                Color::srgba(1.0, 1.0, 1.0, 0.40)
            });
        }
    }

    if let Ok(mut text) = texts.get_mut(ui.hint) {
        **text = context_hint(&local, &node_states, prediction.pos);
    }
    if let Ok(mut text) = texts.get_mut(ui.log) {
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
