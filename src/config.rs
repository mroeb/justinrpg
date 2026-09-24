//! Gameplay constants and pure helpers shared by client and server.
//!
//! NOTE: no `bevy`/`Vec2` here on purpose - this module must stay dependency-free
//! so both binaries compile against the exact same plain data.

pub const PROTOCOL_ID: u64 = 8;
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
/// Kind 0 = Slime, kind 1 = Shadow Wolf, kind 2 = Ember Golem (region mini-boss).
pub fn enemy_hp(kind: u8) -> i32 {
    match kind {
        0 => 100,
        1 => 250,
        _ => 420,
    }
}
pub fn enemy_speed(kind: u8) -> f32 {
    match kind {
        0 => 90.0,
        1 => 145.0,
        _ => 95.0,
    }
}
pub fn enemy_damage(kind: u8) -> i32 {
    match kind {
        0 => 8,
        1 => 16,
        _ => 26,
    }
}
pub fn enemy_name(kind: u8) -> &'static str {
    match kind {
        0 => "Slime",
        1 => "Shadow Wolf",
        _ => "Ember Golem",
    }
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

// ---------------------------------------------------------------------------
// Regions
// ---------------------------------------------------------------------------

/// A named area of the field. `rect` is `[x0, y0, x1, y1]`; the FIRST matching
/// region in `REGIONS` wins, which is why the sanctuary sits at the top.
/// `spawns` lists the enemy kinds that may spawn there (repeats = higher
/// weight); an empty list means monsters never appear there.
#[derive(Clone, Copy)]
pub struct Region {
    pub name: &'static str,
    pub desc: &'static str,
    pub rect: [f32; 4],
    pub spawns: &'static [u8],
}

pub const REGIONS: [Region; 7] = [
    Region {
        name: "Sanctuary of Rebirth",
        desc: "Safe zone: monsters cannot enter and your wounds close fast.",
        rect: [-500.0, -200.0, -100.0, 200.0],
        spawns: &[],
    },
    Region {
        name: "Whisperwood",
        desc: "Ancient trees - gather wood here.",
        rect: [-790.0, 200.0, -110.0, 590.0],
        spawns: &[0, 1],
    },
    Region {
        name: "Emberveins",
        desc: "Gleaming ore veins, guarded by ember golems.",
        rect: [-790.0, -590.0, -110.0, -200.0],
        spawns: &[1, 2],
    },
    Region {
        name: "Homestead",
        desc: "Your farm - till, plant, water, harvest.",
        rect: [150.0, -590.0, 790.0, -70.0],
        spawns: &[],
    },
    Region {
        name: "Stonefall Quarry",
        desc: "Loose boulders - gather stone here.",
        rect: [150.0, -70.0, 790.0, 590.0],
        spawns: &[0, 1],
    },
    Region {
        name: "Gloamfens",
        desc: "A blighted marsh where wolves hunt in packs.",
        rect: [-90.0, -590.0, 140.0, 590.0],
        spawns: &[1, 1, 2],
    },
    // Also acts as the fallback for tiny unassigned gaps between regions.
    Region {
        name: "Western Meadow",
        desc: "Open grassland a stone's throw from the shrine.",
        rect: [-790.0, -190.0, -510.0, 190.0],
        spawns: &[0, 0, 1],
    },
];

/// Index of the region containing (x, y). Unmatched points fall back to the
/// last entry (Western Meadow).
pub fn region_index(x: f32, y: f32) -> usize {
    for (i, r) in REGIONS.iter().enumerate() {
        if x >= r.rect[0] && y >= r.rect[1] && x <= r.rect[2] && y <= r.rect[3] {
            return i;
        }
    }
    REGIONS.len() - 1
}

pub fn region_at(x: f32, y: f32) -> &'static Region {
    &REGIONS[region_index(x, y)]
}

/// Regions that can spawn monsters (used by the server spawner).
pub fn wild_regions() -> Vec<&'static Region> {
    REGIONS.iter().filter(|r| !r.spawns.is_empty()).collect()
}

// ---------------------------------------------------------------------------
// Sanctuary (safe zone at the spawn point)
// ---------------------------------------------------------------------------

pub const SAFE_ZONE_CENTER: [f32; 2] = SPAWN_POINT;
pub const SAFE_ZONE_RADIUS: f32 = 170.0;
/// HP per second restored inside the sanctuary (versus `HP_REGEN_PER_SEC`).
pub const SAFE_REGEN_PER_SEC: f32 = 6.0;

/// True when (x, y) is inside the sanctuary circle.
pub fn in_safe_zone(x: f32, y: f32) -> bool {
    let dx = x - SAFE_ZONE_CENTER[0];
    let dy = y - SAFE_ZONE_CENTER[1];
    dx * dx + dy * dy <= SAFE_ZONE_RADIUS * SAFE_ZONE_RADIUS
}

// ---------------------------------------------------------------------------
// Gatherable resource nodes
// ---------------------------------------------------------------------------

/// Node kinds (also index into `NODE_NAMES` / `NODE_RESOURCE_NAMES`).
pub const NODE_WOOD: u8 = 0;
pub const NODE_STONE: u8 = 1;
pub const NODE_ORE: u8 = 2;

/// `(kind, world position)` for every gatherable node. Positions are a
/// contract: the snapshot's `nodes` vector is index-aligned with this table,
/// exactly like farm plots.
pub const NODE_DEFS: [(u8, [f32; 2]); 18] = [
    // Timber Trees - Whisperwood
    (NODE_WOOD, [-700.0, 520.0]),
    (NODE_WOOD, [-560.0, 300.0]),
    (NODE_WOOD, [-420.0, 480.0]),
    (NODE_WOOD, [-260.0, 260.0]),
    (NODE_WOOD, [-640.0, 400.0]),
    (NODE_WOOD, [-160.0, 530.0]),
    // Boulders - Stonefall Quarry
    (NODE_STONE, [700.0, 520.0]),
    (NODE_STONE, [560.0, 140.0]),
    (NODE_STONE, [300.0, 480.0]),
    (NODE_STONE, [640.0, 280.0]),
    (NODE_STONE, [240.0, 40.0]),
    (NODE_STONE, [430.0, 300.0]),
    // Ore Veins - Emberveins
    (NODE_ORE, [-700.0, -520.0]),
    (NODE_ORE, [-540.0, -260.0]),
    (NODE_ORE, [-380.0, -500.0]),
    (NODE_ORE, [-240.0, -300.0]),
    (NODE_ORE, [-640.0, -380.0]),
    (NODE_ORE, [-160.0, -540.0]),
];

pub const NODE_NAMES: [&str; 3] = ["Timber Tree", "Boulder", "Ore Vein"];
pub const NODE_RESOURCE_NAMES: [&str; 3] = ["wood", "stone", "ore"];
/// Seconds for a depleted node to regrow.
pub const NODE_RESPAWN_TIME: f32 = 7.0;
/// How much a single successful `Gather` yields.
pub const GATHER_AMOUNT: i32 = 2;
/// How close (world units) a player must be to gather from a node.
pub const NODE_RANGE: f32 = 95.0;

pub fn node_count() -> usize {
    NODE_DEFS.len()
}

pub fn node_pos(i: usize) -> [f32; 2] {
    NODE_DEFS[i].1
}

pub fn node_kind(i: usize) -> u8 {
    NODE_DEFS[i].0
}

/// Closest resource node within NODE_RANGE of (x, y), if any.
pub fn nearest_node(x: f32, y: f32) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for i in 0..node_count() {
        let p = node_pos(i);
        let d = (p[0] - x).hypot(p[1] - y);
        if d <= NODE_RANGE && best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, i));
        }
    }
    best.map(|(_, i)| i)
}

// ---------------------------------------------------------------------------
// Crafting: gear bits, recipes, and the derived stats they modify
// ---------------------------------------------------------------------------

pub const GEAR_BLADE: u8 = 1;
pub const GEAR_BELT: u8 = 2;
pub const GEAR_BOOTS: u8 = 4;
pub const GEAR_CHARM: u8 = 8;
pub const GEAR_SHIELD: u8 = 16;

/// A craftable item. `gear == 0` means it is a consumable (brewed into one
/// Healing Draught per craft) instead of a permanent equipment upgrade.
#[derive(Clone, Copy)]
pub struct Recipe {
    pub name: &'static str,
    pub blurb: &'static str,
    pub wood: i32,
    pub stone: i32,
    pub ore: i32,
    pub turnips: i32,
    pub gear: u8,
}

/// Ordered by hotkey: index 0 = key "1", index 1 = key "2", etc.
pub const RECIPES: [Recipe; 6] = [
    Recipe {
        name: "Iron Blade",
        blurb: "+60 one-punch damage",
        wood: 3,
        stone: 2,
        ore: 1,
        turnips: 0,
        gear: GEAR_BLADE,
    },
    Recipe {
        name: "Titan Belt",
        blurb: "+50 max HP",
        wood: 2,
        stone: 4,
        ore: 0,
        turnips: 0,
        gear: GEAR_BELT,
    },
    Recipe {
        name: "Wind Boots",
        blurb: "+70 move speed",
        wood: 4,
        stone: 0,
        ore: 2,
        turnips: 0,
        gear: GEAR_BOOTS,
    },
    Recipe {
        name: "Farmer's Charm",
        blurb: "+1 seed & +1 turnip per harvest",
        wood: 0,
        stone: 3,
        ore: 0,
        turnips: 2,
        gear: GEAR_CHARM,
    },
    Recipe {
        name: "Guardian Shield",
        blurb: "take 30% less damage",
        wood: 2,
        stone: 5,
        ore: 3,
        turnips: 0,
        gear: GEAR_SHIELD,
    },
    Recipe {
        name: "Healing Draught",
        blurb: "drink with Q to restore 50 HP",
        wood: 1,
        stone: 0,
        ore: 1,
        turnips: 1,
        gear: 0,
    },
];

/// HP restored per Healing Draught.
pub const POTION_HEAL: i32 = 50;
/// Fraction of incoming damage ignored while the Guardian Shield is owned.
pub const SHIELD_REDUCTION: f32 = 0.3;
/// Turnips produced by one harvest (farming resource used in recipes).
pub const TURNIPS_PER_HARVEST: i32 = 1;

/// One-punch damage with gear bonuses applied. Both sides must agree on this.
pub fn attack_damage(gear: u8) -> i32 {
    ATTACK_DAMAGE + if gear & GEAR_BLADE != 0 { 60 } else { 0 }
}

pub fn max_hp(gear: u8) -> i32 {
    PLAYER_MAX_HP + if gear & GEAR_BELT != 0 { 50 } else { 0 }
}

pub fn player_speed(gear: u8) -> f32 {
    PLAYER_SPEED + if gear & GEAR_BOOTS != 0 { 70.0 } else { 0.0 }
}

pub fn harvest_seeds(gear: u8) -> i32 {
    SEEDS_PER_HARVEST + if gear & GEAR_CHARM != 0 { 1 } else { 0 }
}

pub fn harvest_turnips(gear: u8) -> i32 {
    TURNIPS_PER_HARVEST + if gear & GEAR_CHARM != 0 { 1 } else { 0 }
}

/// Incoming damage after the Guardian Shield's mitigation.
pub fn damage_taken(gear: u8, raw: i32) -> i32 {
    if gear & GEAR_SHIELD != 0 {
        ((raw as f32) * (1.0 - SHIELD_REDUCTION)).round() as i32
    } else {
        raw
    }
}

/// Whether the given inventory can pay for `r`.
pub fn can_afford(r: &Recipe, wood: i32, stone: i32, ore: i32, turnips: i32) -> bool {
    wood >= r.wood && stone >= r.stone && ore >= r.ore && turnips >= r.turnips
}

/// Compact cost string like `3w 2s 1o`, used by server logs and the crafting UI.
pub fn cost_text(r: &Recipe) -> String {
    let mut parts: Vec<String> = Vec::new();
    if r.wood > 0 {
        parts.push(format!("{}w", r.wood));
    }
    if r.stone > 0 {
        parts.push(format!("{}s", r.stone));
    }
    if r.ore > 0 {
        parts.push(format!("{}o", r.ore));
    }
    if r.turnips > 0 {
        parts.push(format!("{}t", r.turnips));
    }
    if parts.is_empty() {
        "free".to_string()
    } else {
        parts.join(" ")
    }
}

/// Comma-separated names of every equipment bit currently owned.
pub fn gear_names(gear: u8) -> String {
    let mut out = String::new();
    for r in RECIPES.iter().filter(|r| r.gear != 0 && gear & r.gear != 0) {
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(r.name);
    }
    if out.is_empty() {
        "nothing crafted yet".to_string()
    } else {
        out
    }
}
