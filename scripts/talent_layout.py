#!/usr/bin/env python3
"""Export and optimize Rift talent tree layout data.

This is intentionally a data/layout tool, not game runtime logic. It gives us a
CSV surface for reviewing lane membership, branch splits, prerequisite paths, and
synergy placement before we decide whether Rust should consume CSV directly.
"""

from __future__ import annotations

import argparse
import csv
import math
import re
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import Iterable

ROUTE_FILES = ["hub.rs", "warrior.rs", "mage.rs", "healer.rs", "summoner.rs", "synergy.rs"]
CSV_FIELDS = [
    "id",
    "route",
    "lane",
    "tier",
    "name",
    "kind",
    "status",
    "prereqs",
    "x",
    "y",
    "optimized_x",
    "optimized_y",
    "description",
]

ROUTE_DIR = {
    "Warrior": (1.0, 0.0),
    "Mage": (0.0, -1.0),
    "Healer": (0.0, 1.0),
    "Summoner": (-1.0, 0.0),
}

LANE_OFFSETS = {
    "Warrior": {"spine": 0.0, "berserker": -190.0, "vanguard": 230.0, "bridge": 85.0},
    "Mage": {"spine": 0.0, "fire": -260.0, "frost": 260.0, "elemental": 0.0},
    "Healer": {"spine": 0.0, "battle": 260.0, "restoration": -260.0, "harmony": -85.0},
    "Summoner": {"spine": 0.0, "void": 260.0, "corpse": -260.0, "pact": 0.0},
}

ENTRY_DISTANCE = 470.0
TIER_STEP = 125.0
MIN_NODE_SPACING = 96.0
EDGE_CLEARANCE = 62.0
GEOMETRY_ITERATIONS = 120

SYNERGY_PAIR_SLOTS = {
    frozenset(("Warrior", "Mage")): (1.0, -1.0, 1060.0),
    frozenset(("Mage", "Summoner")): (-1.0, -1.0, 1060.0),
    frozenset(("Healer", "Summoner")): (-1.0, 1.0, 1060.0),
    frozenset(("Warrior", "Healer")): (1.0, 1.0, 1060.0),
    frozenset(("Mage", "Healer")): (1.0, -1.0, 1350.0),
    frozenset(("Warrior", "Summoner")): (-1.0, 1.0, 1350.0),
}


@dataclass
class TalentRow:
    id: int
    route: str
    lane: str
    tier: int
    name: str
    kind: str
    status: str
    prereqs: str
    x: float
    y: float
    optimized_x: float
    optimized_y: float
    description: str


def extract_calls(text: str, func: str) -> list[str]:
    calls: list[str] = []
    needle = f"{func}("
    index = 0
    while True:
        start = text.find(needle, index)
        if start < 0:
            break
        if start > 0 and (text[start - 1].isalnum() or text[start - 1] == "_"):
            index = start + len(needle)
            continue
        pos = start + len(func)
        depth = 0
        in_str = False
        escaped = False
        end = pos
        while end < len(text):
            ch = text[end]
            if in_str:
                if escaped:
                    escaped = False
                elif ch == "\\":
                    escaped = True
                elif ch == '"':
                    in_str = False
            else:
                if ch == '"':
                    in_str = True
                elif ch in "([{":
                    depth += 1
                elif ch in ")]}":
                    depth -= 1
                    if depth == 0:
                        calls.append(text[pos + 1 : end])
                        index = end + 1
                        break
            end += 1
        else:
            break
    return calls


def split_args(call: str) -> list[str]:
    args: list[str] = []
    start = 0
    depth = 0
    in_str = False
    escaped = False
    for idx, ch in enumerate(call):
        if in_str:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_str = False
        else:
            if ch == '"':
                in_str = True
            elif ch in "([{":
                depth += 1
            elif ch in ")]}":
                depth -= 1
            elif ch == "," and depth == 0:
                args.append(call[start:idx].strip())
                start = idx + 1
    tail = call[start:].strip()
    if tail:
        args.append(tail)
    return args


def clean_str(value: str) -> str:
    value = value.strip()
    if value.startswith('"') and value.endswith('"'):
        return bytes(value[1:-1], "utf-8").decode("unicode_escape")
    return value


def parse_route(value: str) -> str:
    return value.strip().split("::")[-1]


def parse_prereqs(value: str) -> list[int]:
    return [int(match) for match in re.findall(r"\d+", value)]


def parse_position(value: str) -> tuple[float, float]:
    nums = re.findall(r"-?\d+(?:\.\d+)?", value)
    if len(nums) < 2:
        return (0.0, 0.0)
    return (float(nums[0]), float(nums[1]))


def infer_kind(effect: str, stat_node: bool) -> str:
    if stat_node:
        return "Stat"
    if "UnlockAbility" in effect:
        return "Unlock"
    if "AbilityMod" in effect:
        return "Modifier"
    if "Keystone" in effect:
        return "Keystone"
    if "Synergy" in effect:
        return "Synergy"
    if "PassiveProc" in effect:
        return "PassiveProc"
    return "Other"


def infer_lane(route: str, x: float, y: float, node_id: int) -> str:
    if route == "Hub":
        if 100 <= node_id <= 109:
            return "movement"
        if 110 <= node_id <= 119:
            return "warrior-connector"
        if 210 <= node_id <= 219:
            return "mage-connector"
        if 310 <= node_id <= 319:
            return "healer-connector"
        if 410 <= node_id <= 419:
            return "summoner-connector"
        return "core"
    if route == "Warrior":
        if node_id in (1050, 1051):
            return "bridge"
        if y < -90:
            return "berserker"
        if y > 90:
            return "vanguard"
        return "spine"
    if route == "Mage":
        if node_id in (2050, 2051, 2052):
            return "elemental"
        if x < -90:
            return "fire"
        if x > 90:
            return "frost"
        return "spine"
    if route == "Healer":
        if node_id in (3050, 3051):
            return "harmony"
        if x > 90:
            return "battle"
        if x < -90:
            return "restoration"
        return "spine"
    if route == "Summoner":
        if node_id in (4050, 4051):
            return "pact"
        if y < -90:
            return "void"
        if y > 90:
            return "corpse"
        return "spine"
    if route == "Synergy":
        return "synergy"
    return "unknown"


def export_from_rust(src: Path) -> list[TalentRow]:
    rows: list[TalentRow] = []
    for name in ROUTE_FILES:
        text = (src / name).read_text(encoding="utf-8")
        for call in extract_calls(text, "stat_node"):
            args = split_args(call)
            if len(args) < 11:
                continue
            node_id = int(args[0])
            route = parse_route(args[3])
            prereqs = parse_prereqs(args[7])
            x, y = parse_position(args[10])
            rows.append(
                TalentRow(
                    id=node_id,
                    route=route,
                    lane=infer_lane(route, x, y, node_id),
                    tier=0,
                    name=clean_str(args[1]),
                    kind="Stat",
                    status=args[9].strip(),
                    prereqs="|".join(str(p) for p in prereqs),
                    x=x,
                    y=y,
                    optimized_x=x,
                    optimized_y=y,
                    description=clean_str(args[2]),
                )
            )
        for call in extract_calls(text, "node"):
            args = split_args(call)
            if len(args) < 10:
                continue
            node_id = int(args[0])
            route = parse_route(args[4])
            prereqs = parse_prereqs(args[5])
            x, y = parse_position(args[8])
            rows.append(
                TalentRow(
                    id=node_id,
                    route=route,
                    lane=infer_lane(route, x, y, node_id),
                    tier=0,
                    name=clean_str(args[1]),
                    kind=infer_kind(args[9], False),
                    status=args[7].strip(),
                    prereqs="|".join(str(p) for p in prereqs),
                    x=x,
                    y=y,
                    optimized_x=x,
                    optimized_y=y,
                    description=clean_str(args[2]),
                )
            )
    rows.sort(key=lambda row: row.id)
    assign_tiers(rows)
    return rows


def assign_tiers(rows: list[TalentRow]) -> None:
    by_id = {row.id: row for row in rows}
    memo: dict[int, int] = {}

    def tier(node_id: int) -> int:
        if node_id in memo:
            return memo[node_id]
        row = by_id[node_id]
        prereqs = [int(p) for p in row.prereqs.split("|") if p]
        route_prereqs = [p for p in prereqs if p in by_id and by_id[p].route == row.route]
        if row.route == "Hub" or not route_prereqs:
            value = 0
        else:
            value = 1 + max(tier(p) for p in route_prereqs)
        memo[node_id] = value
        return value

    for row in rows:
        row.tier = tier(row.id)


def read_csv(path: Path) -> list[TalentRow]:
    rows: list[TalentRow] = []
    with path.open(newline="", encoding="utf-8") as handle:
        for raw in csv.DictReader(handle):
            values = {field: raw.get(field, "") for field in CSV_FIELDS}
            rows.append(
                TalentRow(
                    id=int(values["id"]),
                    route=values["route"],
                    lane=values["lane"],
                    tier=int(values["tier"] or 0),
                    name=values["name"],
                    kind=values["kind"],
                    status=values["status"],
                    prereqs=values["prereqs"],
                    x=float(values["x"] or 0.0),
                    y=float(values["y"] or 0.0),
                    optimized_x=float(values["optimized_x"] or values["x"] or 0.0),
                    optimized_y=float(values["optimized_y"] or values["y"] or 0.0),
                    description=values["description"],
                )
            )
    return rows


def write_csv(path: Path, rows: Iterable[TalentRow]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=CSV_FIELDS)
        writer.writeheader()
        for row in sorted(rows, key=lambda item: item.id):
            data = asdict(row)
            data["x"] = f"{row.x:.1f}"
            data["y"] = f"{row.y:.1f}"
            data["optimized_x"] = f"{row.optimized_x:.1f}"
            data["optimized_y"] = f"{row.optimized_y:.1f}"
            writer.writerow(data)


def optimize(rows: list[TalentRow]) -> list[TalentRow]:
    assign_tiers(rows)
    by_id = {row.id: row for row in rows}

    for row in rows:
        if row.route == "Hub":
            row.optimized_x = row.x
            row.optimized_y = row.y
            continue
        if row.route in ROUTE_DIR:
            dx, dy = ROUTE_DIR[row.route]
            nx, ny = -dy, dx
            lane_offsets = LANE_OFFSETS[row.route]
            lane_offset = lane_offsets.get(row.lane, 0.0)
            distance = ENTRY_DISTANCE + row.tier * TIER_STEP
            if row.kind == "Keystone":
                distance += 80.0
            if row.kind == "Modifier":
                distance += 24.0
            row.optimized_x = dx * distance + nx * lane_offset
            row.optimized_y = dy * distance + ny * lane_offset

    spread_same_lane_siblings(rows)

    synergy_rows = [row for row in rows if row.route == "Synergy"]
    pair_slots: dict[frozenset[str], list[TalentRow]] = {}
    for row in synergy_rows:
        parents = [by_id[int(p)] for p in row.prereqs.split("|") if p and int(p) in by_id]
        pair_key = frozenset(parent.route for parent in parents if parent.route in ROUTE_DIR)
        pair_slots.setdefault(pair_key, []).append(row)

    pair_indices: dict[int, tuple[int, int]] = {}
    for group in pair_slots.values():
        group.sort(key=lambda item: item.id)
        for index, row in enumerate(group):
            pair_indices[row.id] = (index, len(group))

    for index, row in enumerate(synergy_rows):
        parents = [by_id[int(p)] for p in row.prereqs.split("|") if p and int(p) in by_id]
        if not parents:
            continue
        pair_index, pair_count = pair_indices.get(row.id, (index, 1))
        sx, sy = synergy_anchor(row, parents, index, pair_index, pair_count)
        row.optimized_x = sx
        row.optimized_y = sy

    relax_geometry(rows)

    return rows


def synergy_anchor(
    row: TalentRow,
    parents: list[TalentRow],
    index: int,
    pair_index: int,
    pair_count: int,
) -> tuple[float, float]:
    parent_routes = frozenset(parent.route for parent in parents if parent.route in ROUTE_DIR)
    if parent_routes in SYNERGY_PAIR_SLOTS:
        sx, sy, distance = SYNERGY_PAIR_SLOTS[parent_routes]
        length = math.hypot(sx, sy) or 1.0
        nx = sx / length
        ny = sy / length
        tangent = (-ny, nx)
        midpoint = (pair_count - 1) * 0.5
        stagger = (pair_index - midpoint) * 155.0
        ring = distance + (pair_index % 2) * 36.0
        return (nx * ring + tangent[0] * stagger, ny * ring + tangent[1] * stagger)

    avg_x = sum(parent.optimized_x for parent in parents) / len(parents)
    avg_y = sum(parent.optimized_y for parent in parents) / len(parents)
    length = math.hypot(avg_x, avg_y)

    if len(parents) >= 2:
        ax, ay = parents[0].optimized_x, parents[0].optimized_y
        bx, by = parents[1].optimized_x, parents[1].optimized_y
        dot = ax * bx + ay * by
        parent_len = (math.hypot(ax, ay) * math.hypot(bx, by)) or 1.0
        if dot / parent_len < -0.35:
            dx = bx - ax
            dy = by - ay
            edge_len = math.hypot(dx, dy) or 1.0
            nx = -dy / edge_len
            ny = dx / edge_len
            if nx * avg_x + ny * avg_y < 0.0:
                nx = -nx
                ny = -ny
            if abs(avg_x) + abs(avg_y) < 260.0:
                if row.id % 2 == 0:
                    nx, ny = (1.0, 0.0)
                else:
                    nx, ny = (-1.0, 0.0)
            distance = 920.0 + (index % 3) * 120.0
            tangent = (-ny, nx)
            stagger = ((index % 5) - 2) * 64.0
            return (nx * distance + tangent[0] * stagger, ny * distance + tangent[1] * stagger)

    if length < 1.0:
        length = 1.0
        avg_x = 1.0
        avg_y = 0.0
    outward = 140.0 + (index % 3) * 55.0
    tangent = (-avg_y / length, avg_x / length)
    stagger = ((index % 5) - 2) * 36.0
    return (avg_x + (avg_x / length) * outward + tangent[0] * stagger, avg_y + (avg_y / length) * outward + tangent[1] * stagger)


def spread_same_lane_siblings(rows: list[TalentRow]) -> None:
    groups: dict[tuple[str, str, int], list[TalentRow]] = {}
    for row in rows:
        if row.route not in ROUTE_DIR:
            continue
        groups.setdefault((row.route, row.lane, row.tier), []).append(row)

    for (route, _lane, _tier), group in groups.items():
        if len(group) <= 1:
            continue
        group.sort(key=lambda row: (row.kind != "Stat", row.id))
        dx, dy = ROUTE_DIR[route]
        nx, ny = -dy, dx
        midpoint = (len(group) - 1) * 0.5
        for slot, row in enumerate(group):
            spread = (slot - midpoint) * 58.0
            row.optimized_x += nx * spread
            row.optimized_y += ny * spread


def prereq_ids(row: TalentRow) -> list[int]:
    return [int(value) for value in row.prereqs.split("|") if value]


def dist_point_segment(px: float, py: float, ax: float, ay: float, bx: float, by: float) -> tuple[float, float, float, float]:
    dx = bx - ax
    dy = by - ay
    den = dx * dx + dy * dy
    if den <= 0.0001:
        return (math.hypot(px - ax, py - ay), ax, ay, 0.0)
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / den))
    qx = ax + dx * t
    qy = ay + dy * t
    return (math.hypot(px - qx, py - qy), qx, qy, t)


def clamp(value: float, limit: float) -> float:
    return max(-limit, min(limit, value))


def relax_geometry(rows: list[TalentRow]) -> None:
    by_id = {row.id: row for row in rows}
    anchors = {row.id: (row.optimized_x, row.optimized_y) for row in rows}
    movable = [row for row in rows if row.route != "Hub"]
    edges = [
        (by_id[parent_id], row)
        for row in rows
        if row.route != "Synergy"
        for parent_id in prereq_ids(row)
        if parent_id in by_id
    ]

    for _ in range(GEOMETRY_ITERATIONS):
        deltas = {row.id: [0.0, 0.0] for row in movable}

        for i, a in enumerate(movable):
            for b in movable[i + 1 :]:
                dx = b.optimized_x - a.optimized_x
                dy = b.optimized_y - a.optimized_y
                distance = math.hypot(dx, dy) or 1.0
                if distance >= MIN_NODE_SPACING:
                    continue
                push = (MIN_NODE_SPACING - distance) * 0.18
                nx = dx / distance
                ny = dy / distance
                deltas[a.id][0] -= nx * push
                deltas[a.id][1] -= ny * push
                deltas[b.id][0] += nx * push
                deltas[b.id][1] += ny * push

        for parent, child in edges:
            ax, ay = parent.optimized_x, parent.optimized_y
            bx, by = child.optimized_x, child.optimized_y
            for node in movable:
                if node.id in (parent.id, child.id):
                    continue
                distance, qx, qy, t = dist_point_segment(node.optimized_x, node.optimized_y, ax, ay, bx, by)
                if t <= 0.08 or t >= 0.92 or distance >= EDGE_CLEARANCE:
                    continue
                nx = node.optimized_x - qx
                ny = node.optimized_y - qy
                length = math.hypot(nx, ny)
                if length <= 0.001:
                    edge_dx = bx - ax
                    edge_dy = by - ay
                    edge_len = math.hypot(edge_dx, edge_dy) or 1.0
                    nx = -edge_dy / edge_len
                    ny = edge_dx / edge_len
                else:
                    nx /= length
                    ny /= length
                push = (EDGE_CLEARANCE - distance) * 0.22
                deltas[node.id][0] += nx * push
                deltas[node.id][1] += ny * push

        for node in movable:
            anchor_x, anchor_y = anchors[node.id]
            spring = 0.11 if node.route == "Synergy" else 0.035
            deltas[node.id][0] += (anchor_x - node.optimized_x) * spring
            deltas[node.id][1] += (anchor_y - node.optimized_y) * spring

        for node in movable:
            node.optimized_x += clamp(deltas[node.id][0], 14.0)
            node.optimized_y += clamp(deltas[node.id][1], 14.0)


def geometry_issues(rows: list[TalentRow]) -> tuple[int, int]:
    by_id = {row.id: row for row in rows}
    node_hits = 0
    edge_hits = 0
    for i, a in enumerate(rows):
        for b in rows[i + 1 :]:
            if math.hypot(a.optimized_x - b.optimized_x, a.optimized_y - b.optimized_y) < MIN_NODE_SPACING:
                node_hits += 1
    for row in rows:
        if row.route == "Synergy":
            continue
        for parent_id in prereq_ids(row):
            parent = by_id.get(parent_id)
            if parent is None:
                continue
            for node in rows:
                if node.id in (parent.id, row.id):
                    continue
                distance, _qx, _qy, t = dist_point_segment(
                    node.optimized_x,
                    node.optimized_y,
                    parent.optimized_x,
                    parent.optimized_y,
                    row.optimized_x,
                    row.optimized_y,
                )
                if 0.08 < t < 0.92 and distance < EDGE_CLEARANCE:
                    edge_hits += 1
    return node_hits, edge_hits


def report(rows: list[TalentRow]) -> None:
    print(f"nodes: {len(rows)}")
    for route in ["Hub", "Warrior", "Mage", "Healer", "Summoner", "Synergy"]:
        route_rows = [row for row in rows if row.route == route]
        if not route_rows:
            continue
        lanes = sorted({row.lane for row in route_rows})
        print(f"{route}: {len(route_rows)} nodes; lanes: {', '.join(lanes)}")
    min_x = min(row.optimized_x for row in rows)
    max_x = max(row.optimized_x for row in rows)
    min_y = min(row.optimized_y for row in rows)
    max_y = max(row.optimized_y for row in rows)
    node_hits, edge_hits = geometry_issues(rows)
    print(f"optimized bounds: x={min_x:.0f}..{max_x:.0f}, y={min_y:.0f}..{max_y:.0f}")
    print(f"layout issues under target clearance: node-node={node_hits}, edge-node={edge_hits}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    export = sub.add_parser("export-rust", help="export current Rust-authored talent nodes into CSV")
    export.add_argument("--src", type=Path, default=Path("crates/rift-game/src/talents"))
    export.add_argument("--out", type=Path, default=Path("assets/talents/talent_tree_layout.csv"))

    opt = sub.add_parser("optimize", help="compute optimized_x/optimized_y from route lanes and prerequisites")
    opt.add_argument("--csv", type=Path, default=Path("assets/talents/talent_tree_layout.csv"))
    opt.add_argument("--out", type=Path, default=Path("assets/talents/talent_tree_layout.optimized.csv"))

    args = parser.parse_args()
    if args.cmd == "export-rust":
        rows = export_from_rust(args.src)
        rows = optimize(rows)
        write_csv(args.out, rows)
        report(rows)
    elif args.cmd == "optimize":
        rows = read_csv(args.csv)
        rows = optimize(rows)
        write_csv(args.out, rows)
        report(rows)


if __name__ == "__main__":
    main()
