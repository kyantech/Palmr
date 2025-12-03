#!/usr/bin/env python3
"""
Prune extra translation keys using en-US.json as the reference.
Removes keys that exist in target language files but not in the reference.

Usage examples:
  python3 prune_translations.py --dry-run
  python3 prune_translations.py --messages-dir ../messages
  python3 prune_translations.py --reference en-US.json
"""

import json
from pathlib import Path
from typing import Dict, Any, Set, List, Tuple
import argparse


def load_json_file(file_path: Path) -> Dict[str, Any]:
    try:
        with open(file_path, 'r', encoding='utf-8') as f:
            return json.load(f)
    except Exception as e:
        print(f"Error loading {file_path}: {e}")
        return {}


def save_json_file(file_path: Path, data: Dict[str, Any], indent: int = 2) -> bool:
    try:
        with open(file_path, 'w', encoding='utf-8') as f:
            json.dump(data, f, ensure_ascii=False, indent=indent, separators=(',', ': '))
            f.write('\n')
        return True
    except Exception as e:
        print(f"Error saving {file_path}: {e}")
        return False


def get_all_keys(data: Dict[str, Any], prefix: str = '') -> Set[str]:
    keys: Set[str] = set()
    for key, value in data.items():
        current_key = f"{prefix}.{key}" if prefix else key
        keys.add(current_key)
        if isinstance(value, dict):
            keys.update(get_all_keys(value, current_key))
    return keys


def delete_nested_key(data: Dict[str, Any], key_path: str) -> bool:
    """Delete a nested key using a dotted path. Returns True if deleted."""
    keys = key_path.split('.')
    current: Any = data
    for key in keys[:-1]:
        if not isinstance(current, dict) or key not in current:
            return False
        current = current[key]
    last = keys[-1]
    if isinstance(current, dict) and last in current:
        del current[last]
        return True
    return False


def prune_file(reference: Dict[str, Any], target: Dict[str, Any]) -> Tuple[Dict[str, Any], List[str]]:
    ref_keys = get_all_keys(reference)
    tgt_keys = get_all_keys(target)
    extras = sorted(list(tgt_keys - ref_keys))

    if not extras:
        return target, []

    # Work on a copy
    pruned = json.loads(json.dumps(target))
    for key in extras:
        delete_nested_key(pruned, key)
    return pruned, extras


def prune_translations(messages_dir: Path, reference_file: str = 'en-US.json', dry_run: bool = False) -> int:
    reference_path = messages_dir / reference_file
    if not reference_path.exists():
        print(f"Reference file not found: {reference_path}")
        return 1

    print(f"Loading reference: {reference_file}")
    ref = load_json_file(reference_path)
    if not ref:
        print("Error loading reference file")
        return 1

    files = [p for p in messages_dir.glob('*.json') if p.name != reference_file]
    if not files:
        print("No translation files found")
        return 0

    ref_count = len(get_all_keys(ref))
    print(f"Reference keys: {ref_count}")
    print(f"Processing {len(files)} files...\n")

    total_removed = 0
    changed_files = 0

    for p in sorted(files):
        data = load_json_file(p)
        if not data:
            print(f"{p.name}: ❌ load error")
            continue
        before = len(get_all_keys(data))
        pruned, extras = prune_file(ref, data)
        after = len(get_all_keys(pruned))
        if not extras:
            print(f"{p.name}: ✅ no extras ({before}/{ref_count})")
            continue

        print(f"{p.name}: 🧹 removing {len(extras)} extra key(s)")
        # Optionally show first few extras
        for k in extras[:5]:
            print(f"   - {k}")
        if len(extras) > 5:
            print(f"   ... and {len(extras) - 5} more")

        total_removed += len(extras)
        changed_files += 1
        if not dry_run:
            if save_json_file(p, pruned):
                print(f"   ✅ saved ({after}/{ref_count})")
            else:
                print(f"   ❌ save error")
        else:
            print(f"   📝 [DRY RUN] not saved")
        print()

    print("=" * 60)
    print("SUMMARY")
    print("=" * 60)
    print(f"Files changed: {changed_files}")
    print(f"Extra keys removed: {total_removed}")
    if dry_run:
        print("Mode: DRY RUN")
    return 0


def main():
    parser = argparse.ArgumentParser(description='Prune extra translation keys using reference file')
    parser.add_argument('--messages-dir', type=Path, default=Path(__file__).parent.parent / 'messages', help='Messages directory')
    parser.add_argument('--reference', default='en-US.json', help='Reference filename')
    parser.add_argument('--dry-run', action='store_true', help='Only show what would be removed')
    args = parser.parse_args()

    if not args.messages_dir.exists():
        print(f"Directory not found: {args.messages_dir}")
        return 1

    return prune_translations(args.messages_dir, args.reference, args.dry_run)


if __name__ == '__main__':
    exit(main())
