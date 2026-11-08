#!/usr/bin/env python3
"""A packets report (the shape of the data generator's packets.json) from decompiled network protocol sources.

For a release whose data generator cannot be run (no server jar yet), the ids still follow from the code: a
connection state's packets are numbered in the order its *Protocols.java registers them. Decompile
`net/minecraft/network/protocol/**` of the (remapped, see docs/mc-1.21.11.md) client jar with Vineflower, then

  packets_from_sources.py DECOMPILED_DIR OUT.json
  generate_packet_ids.py --report OUT.json --jar CLIENT.jar --out crates/horizon-protocol/src/ids_1_21_11.rs

The data generator's own report (`--reports`) is authoritative: compare with it once it can be run.
"""
import json
import re
import sys
from pathlib import Path

STATE_NAMES = {"game": "play"}  # the package is `game`, the connection state `play`
DECLARATION = re.compile(
    r"public static final PacketType<[^>]*(?:<[^>]*>)?[^>]*> (\w+) = \w+\(\s*\"([a-z_0-9/]+)\"\s*\)")
REGISTRATION = re.compile(r"\.(addPacket|withBundlePacket)\((\w+)\.(\w+)")


def packet_type_names(root):
    names = {}
    for file in root.rglob("*PacketTypes.java"):
        for match in DECLARATION.finditer(file.read_text()):
            names[(file.stem, match.group(1))] = match.group(2)
    return names


def report(root):
    names = packet_type_names(root)
    out = {}
    for file in sorted(root.rglob("*Protocols.java")):
        state = STATE_NAMES.get(file.parent.name, file.parent.name)
        for kind, cls, const in REGISTRATION.findall(file.read_text()):
            if kind == "withBundlePacket":
                # The bundle packet itself is never sent: its delimiter is what is registered.
                const += "_DELIMITER"
            direction = "clientbound" if const.startswith("CLIENTBOUND_") else "serverbound"
            entries = out.setdefault(state, {}).setdefault(direction, {})
            entries[f"minecraft:{names[(cls, const)]}"] = {"protocol_id": len(entries)}
    return out


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    Path(sys.argv[2]).write_text(json.dumps(report(Path(sys.argv[1])), indent=2) + "\n")
