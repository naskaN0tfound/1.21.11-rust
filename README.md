# 1.21.11-rust
A project for 1.21.11 made in rust inspired by horion

Port of [horizon-rust/horizon](https://github.com/horizon-rust/horizon) (Minecraft: Java Edition
rewritten in Rust, MIT) from 26.3 back to 1.21.11. Status: **planning**. The game itself is not
ported yet; see [PORTING_PLAN.md](PORTING_PLAN.md).

## Launcher

`launcher` is a small std-only Rust program that finds the game executable, creates a game folder
and runs it there.

```
cargo run -- --check                       # report what it would run
cargo run -- --game-bin PATH --game-dir DIR -- <game args>
```

The game binary can also come from `RUST121_BIN`. Until the 1.21.11 client exists the launcher
reports that it is missing.

## Rules

- Never commit Mojang's files (jars, assets, decompiled source, prepared resources).
- Keep upstream's MIT license and attribution.
- Regenerate version constants (protocol, data version, pack formats) from your own 1.21.11 jar
  instead of copying numbers from memory.
