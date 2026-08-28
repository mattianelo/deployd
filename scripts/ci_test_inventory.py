#!/usr/bin/env python3
import argparse
import re
import sys
import tomllib
from collections import defaultdict
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
INVENTORY = ROOT / "ci" / "test-inventory.toml"
VARIANTS = {"appimage", "snap", "both"}
ENGINES = {"bethesda", "aurora", "eclipse", "redengine"}
CASES = {"positive", "negative"}
INTERFACE_STATES = {"connected", "disconnected", "not_applicable"}
TEST_RE = re.compile(r"\b(?:async\s+)?fn\s+([a-zA-Z0-9_]+)\s*\(")
VARIANT_RE = re.compile(r"@variants:\s*([a-zA-Z0-9_-]+)")


class InventoryError(RuntimeError):
    pass


def load_inventory(path: Path = INVENTORY) -> dict:
    with path.open("rb") as stream:
        return tomllib.load(stream)


def test_exists(root: Path, relative_path: str, name: str) -> bool:
    source = root / relative_path
    if not source.is_file():
        return False
    return re.search(rf"\b(?:async\s+)?fn\s+{re.escape(name)}\s*\(", source.read_text()) is not None


def source_variant_tags(root: Path) -> dict[tuple[str, str], str]:
    tags: dict[tuple[str, str], str] = {}
    for source in sorted((root / "src").rglob("*.rs")):
        lines = source.read_text().splitlines()
        for index, line in enumerate(lines):
            match = VARIANT_RE.search(line)
            if match is None:
                continue
            for candidate in lines[index + 1 : index + 5]:
                function = TEST_RE.search(candidate)
                if function is not None:
                    relative = source.relative_to(root).as_posix()
                    tags[(relative, function.group(1))] = match.group(1)
                    break
            else:
                raise InventoryError(f"variant tag is not attached to a test: {source}:{index + 1}")
    return tags


def validate_inventory(root: Path = ROOT, inventory_path: Path = INVENTORY) -> dict:
    data = load_inventory(inventory_path)
    declared_variants: dict[tuple[str, str], str] = {}
    interface_states: dict[str, set[str]] = defaultdict(set)

    for entry in data.get("variant_tests", []):
        path = entry.get("path", "")
        name = entry.get("name", "")
        variant = entry.get("variant")
        state = entry.get("interface_state")
        key = (path, name)
        if key in declared_variants:
            raise InventoryError(f"duplicate variant test: {path}::{name}")
        if variant not in VARIANTS:
            raise InventoryError(f"invalid variant for {path}::{name}: {variant}")
        if state not in INTERFACE_STATES:
            raise InventoryError(f"invalid interface state for {path}::{name}: {state}")
        interface = entry.get("interface")
        if state == "not_applicable" and interface is not None:
            raise InventoryError(f"not-applicable interface test names an interface: {path}::{name}")
        if state != "not_applicable" and not interface:
            raise InventoryError(f"interface-sensitive test omits its interface: {path}::{name}")
        if interface:
            interface_states[interface].add(state)
        if not test_exists(root, path, name):
            raise InventoryError(f"variant test does not exist: {path}::{name}")
        declared_variants[key] = variant

    actual_variants = source_variant_tags(root)
    if declared_variants != actual_variants:
        missing = sorted(actual_variants.keys() - declared_variants.keys())
        stale = sorted(declared_variants.keys() - actual_variants.keys())
        changed = sorted(
            key
            for key in actual_variants.keys() & declared_variants.keys()
            if actual_variants[key] != declared_variants[key]
        )
        raise InventoryError(
            f"variant inventory drift; missing={missing}, stale={stale}, changed={changed}"
        )

    for interface, states in interface_states.items():
        if states != {"connected", "disconnected"}:
            raise InventoryError(f"interface {interface} lacks paired coverage: {sorted(states)}")

    engine_cases: dict[str, set[str]] = defaultdict(set)
    seen_engine_entries: set[tuple[str, str, str, str]] = set()
    for entry in data.get("engine_tests", []):
        path = entry.get("path", "")
        name = entry.get("name", "")
        engine = entry.get("engine")
        case = entry.get("case")
        key = (path, name, engine, case)
        if key in seen_engine_entries:
            raise InventoryError(f"duplicate engine test: {key}")
        if engine not in ENGINES or case not in CASES:
            raise InventoryError(f"invalid engine classification: {key}")
        if not test_exists(root, path, name):
            raise InventoryError(f"engine test does not exist: {path}::{name}")
        seen_engine_entries.add(key)
        engine_cases[engine].add(case)

    for engine in ENGINES:
        if engine_cases[engine] != CASES:
            raise InventoryError(
                f"engine {engine} lacks positive and negative coverage: {sorted(engine_cases[engine])}"
            )
    return data


def filter_expression(group: str, data: dict) -> str:
    category, separator, value = group.partition(":")
    if not separator:
        raise InventoryError(f"invalid inventory group: {group}")
    if category == "variant" and value in {"appimage", "snap"}:
        names = {
            entry["name"]
            for entry in data["variant_tests"]
            if entry["variant"] in {value, "both"}
        }
    elif category == "engine" and value in ENGINES:
        names = {
            entry["name"]
            for entry in data["engine_tests"]
            if entry["engine"] == value
        }
    else:
        raise InventoryError(f"invalid inventory group: {group}")
    if not names:
        raise InventoryError(f"inventory group is empty: {group}")
    return " | ".join(f"test({name})" for name in sorted(names))


def main() -> int:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("validate")
    filter_parser = subparsers.add_parser("filter")
    filter_parser.add_argument("group")
    args = parser.parse_args()

    try:
        data = validate_inventory()
        if args.command == "filter":
            print(filter_expression(args.group, data))
        else:
            print("test inventory validation passed")
    except InventoryError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
