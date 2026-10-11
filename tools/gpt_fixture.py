#!/usr/bin/env python3
"""Create only small scratch fixtures from the actual read-only GPT planner codec."""
import json
from pathlib import Path
import uuid
import gpt_plan

ROOT = Path(__file__).resolve().parents[1]


def main():
    destination = ROOT / 'work/gpt-fixtures'
    destination.mkdir(parents=True, exist_ok=True)
    for size in (512, 4096):
        capacity = 2 * 1024 * 1024
        parts = gpt_plan.plan(capacity, size, alignment=4096, esp=65536,
                              system=65536, recovery=65536, minimum_data=65536)
        # Deterministic fixture IDs; these are public disk identifiers, not keys.
        parts = [gpt_plan.Partition(p.name, p.type_guid, uuid.UUID(int=index + 1),
                                    p.first, p.last, p.attributes) for index, p in enumerate(parts)]
        metadata = gpt_plan.encode(capacity, size, uuid.UUID(int=100), parts)
        target = destination / f'gpt-{size}.img'
        with target.open('wb') as stream:
            stream.truncate(capacity)
            for lba, payload in sorted(metadata.items()):
                stream.seek(lba * size); stream.write(payload)
        with target.open('rb') as stream:
            report = gpt_plan.inspect(lambda at, length: (stream.seek(at), stream.read(length))[1],
                                      capacity, size)
        (destination / f'gpt-{size}.json').write_text(json.dumps(report, indent=2) + '\n')
    print('Built planner-encoded 512/4096-byte GPT scratch fixtures; no boot image changed')


if __name__ == '__main__':
    main()
