# Credits

This project exists because of other people's work. Thank you to:

- **Horizon**, by Ludovic Marie and the Horizon contributors: the Rust Minecraft client and launcher this
  repository is built on. MIT licence, kept in `LICENSE`; its readme is `README.horizon.md` and its roadmap and
  notices are `ROADMAP.md` and `THIRD_PARTY_NOTICES.md`. The comparison-test method (Java oracles run on the vanilla
  jars) is Horizon's.
- **Mojang Studios and Microsoft**: Minecraft: Java Edition, including its 1.21.11 client and server jars and the
  official obfuscation mappings (`client.txt`, `server.txt`). The remapper in `crates/horizon-prepare/src/remap.rs`
  reads those mappings from files you download yourself; none of Mojang's files are in this repository.
  Minecraft is a trademark of Mojang AB. This project is not affiliated with Mojang or Microsoft.
- **Vineflower** (decompiler), used on the machine that prepares the game's data.
- **OpenJDK and ASM** (the bytecode library inside the JDK): `tools/remap/Remap.java` uses it as an independent
  second remapper to check the Rust one against.
- **The Rust crates** the launcher and game use, with their licences, in `THIRD_PARTY_NOTICES.md` and `Cargo.lock`.
- **Naska** (naskaN0tfound): the Sparkz launcher idea and name, the 1.21.11 goal, and the 1.21.11 client, server and
  mapping files and version manifest that the work was checked against.
- **Claude** (Anthropic, Claude Sonnet 5.5): wrote the 1.21.11 port, the remapper, the packet tooling and the build
  scripts in this repository, with Naska directing.
