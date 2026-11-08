# 1.21.11-rust

Minecraft Java **1.21.11** in Rust, plus the **Sparkz** launcher (26.3 and 1.21.11).

Built on [Horizon](https://github.com/) by Ludovic Marie and the Horizon contributors (MIT, see `LICENSE`
and `THIRD_PARTY_NOTICES.md`). Horizon's own readme is `README.horizon.md`.

## Status
The 1.21.11 build compiles and its data prepares, but **no comparison test has passed against vanilla 1.21.11
yet**, so it is not exactly 1.21.11. The full state and work list are in `docs/mc-1.21.11.md`.

```
cargo build --release -p horizon-client --features mc-1-21-11     # the 1.21.11 game
```

## Launcher (Windows)
Run `build-sparkz.bat` from the folder that contains this repo's `launcher` folder. It installs Rust and the C++
build tools, builds the launcher, and writes `Sparkz.exe` beside the `.bat`. Microsoft sign-in needs your own
Azure client ID and Mojang's approval: see `SPARKZ-LOGIN.txt`.
