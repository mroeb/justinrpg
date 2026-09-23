# JustinRPG

Isekai-flavored top-down multiplayer RPG: you are the overpowered protagonist,
reincarnated into a world of slimes and farmland. One-punch the monsters, farm
Divine Turnips, show off your harvest count to other players.

Rust + Bevy 0.19, client/server multiplayer over UDP (renet).

## Run

```sh
# terminal 1 - headless server (listens on 127.0.0.1:5000/UDP)
cargo run --bin server

# terminal 2..n - clients (each window is a player)
cargo run
```

First build is slow (Bevy from scratch); later builds are incremental.

## Controls

- **WASD / arrows** - move
- **Space** - one-punch attack (AoE)
- **E** - farm context action on the nearest plot: till -> plant -> water -> harvest

Harvesting yields seeds; killing monsters sometimes drops seeds. Slimes die in
one hit; Shadow Wolves do not.
