# AGENTS.md

## What this is

Isekai-themed top-down **multiplayer** RPG (Rust + Bevy): one-punch combat + farming.
Single crate, client/server over UDP (renet). No assets — all visuals are colored
sprites and the default font; there is no asset pipeline, codegen, or migrations.

## Commands

```sh
cargo run --bin server   # headless server, 127.0.0.1:5000/UDP (start first)
cargo run                # client window (default-run = "justin-rpg"); run several for multiplayer
cargo check --all-targets # fast verification (lint/test tooling is not set up)
```

- First build is slow: dev profile is `opt-level = 1` with deps at `opt-level = 3`
  (Bevy is unplayable otherwise). Don't "optimize" this away.
- No test suite exists. `cargo check`/`cargo build` is the verification step.

## Architecture

- `src/lib.rs` → two modules shared by BOTH binaries:
  - `src/config.rs`: tuning constants + pure helpers (field size, plot layout,
    speeds, ranges, tick rate). Deliberately has **no bevy/glam imports** — keep it that way.
  - `src/protocol.rs`: `C2S` / `S2C` bincode messages. Changing this affects both sides.
- `src/bin/server.rs`: headless (`MinimalPlugins`), **server-authoritative**.
- `src/main.rs`: client — rendering, input, client-side prediction.

### Networking rules (easy to get wrong)

- Server sim is a fixed **30 Hz manual accumulator** inside `server_update` — NOT
  Bevy's `FixedUpdate`. One snapshot per step.
- Channels: `Unreliable` = movement + snapshots; `ReliableOrdered` = actions + logs.
  Edge-triggered actions (attack/farm) MUST stay on the reliable channel or they
  get dropped.
- Snapshots include all players; the client skips its own `id` for entity sync and
  instead corrects its prediction (hard snap >150 units, ease >30 units).
- `plots` in a snapshot is row-major and must match `config::plot_pos()` indexing
  on the client — ordering is the contract, there are no plot ids. Same for
  `nodes` vs `config::node_pos()` (gatherable resource nodes).
- Client predicts its own movement using `config` constants; server applies the
  same constants. Changing a constant on one side only causes rubber-banding.
- Protocol id (`config::PROTOCOL_ID`) must match on both sides or connections fail.

## Version pins

- `bevy = "0.19"` and `bevy_renet = "5.0"` are a matched pair — bump them together
  (bevy_renet 5.x = bevy 0.19, 4.x = 0.18, ...).
- Bevy 0.19 quirks in use: `TextFont.font_size` is `FontSize::Px(..)`, netcode/
  renet events are observed with `On<...>` (not `EventReader`), `bevy_renet`'s own
  README examples are stale — trust bevy 0.19 examples and `bevy_renet/examples/simple.rs`.
- Crate is edition **2021**: `let`-chains (`if let A = .. && let B = ..`) are NOT
  allowed here even though newer rustc supports them elsewhere.

## Gameplay loop (where to work)

Server owns all rules in `step()` + `handle_action()`: movement clamp, one-punch
AoE attack, enemy chase/attack AI, spawner, crop growth (till → plant → water →
harvest), HP/death/respawn, log broadcasts. Client is deliberately thin: it only
renders snapshots, predicts itself, and shows `GameLog` lines.

Additional systems (all server-authoritative, constants in `config.rs`):

- **Regions**: `config::REGIONS` — 7 named rects, first match wins (sanctuary
  listed first). A region decides the enemy spawn mix; region changes broadcast
  as log lines; the client renders overlays + labels from the same table.
- **Safe zone**: circle at spawn (`in_safe_zone`). Enemies never target or enter
  it, damage is blocked inside, and regen is boosted (`SAFE_REGEN_PER_SEC`).
- **Gathering**: `ActionKind::Gather` (key G) on `config::NODE_DEFS` nodes; nodes
  deplete and regrow server-side (`NODE_RESPAWN_TIME`), readiness in snapshots.
- **Crafting**: `ActionKind::Craft(id)` (keys 1-6) + `Drink` (key Q). Recipes and
  derived stats (`attack_damage` / `max_hp` / `player_speed` / `damage_taken`
  over the player's `gear` bitmask) live in `config.rs` — client prediction must
  use the same gear-scaled speed or movement rubber-bands.
