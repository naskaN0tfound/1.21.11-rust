# Porting plan: 26.3 to 1.21.11

Each stage should end with the build green and upstream's differential tests (rerun against
1.21.11) as the acceptance check.

1. **Fork and baseline.** Fork upstream, confirm `cargo check` for the workspace, run the existing
   tests, record what passes at 26.3.
2. **Version pins.** Regenerate `horizon-core/src/version.rs` from a 1.21.11 jar via
   `tools/gen_version_consts.py`. Update `horizon-prepare` to build `.local/horizon-client/1.21.11`.
3. **Protocol.** Rework `horizon-protocol` packets (handshake, status, login, configuration, play)
   and entity data to the 1.21.11 protocol.
4. **Data and registries.** Blocks, items, item components, entities, sounds, particles, tags,
   recipes and loot to match 1.21.11. Remove content added after 1.21.11.
5. **World.** Datafixers (`horizon-world/datafix`), world generation and level logic for 1.21.11
   behaviour. Check terrain against the Java game.
6. **Client.** Assets, models, shaders, lang keys and menus for 1.21.11.
7. **Launcher and packaging.** Separate Cargo workspace in `launcher/`: version picker, install and
   update flow, release signing.
8. **Verification.** Differential tests against vanilla 1.21.11, then play-testing.

## Suggested issues

- Fork upstream and record a 26.3 baseline
- Regenerate version constants for 1.21.11
- Port the protocol layer (one issue per state: handshake/status/login, configuration, play)
- Port registries and item components
- Port datafixers and world generation
- Port client assets and UI
- Port the launcher
- Differential test harness for 1.21.11
