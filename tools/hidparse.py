#!/usr/bin/env python3
"""Parse a USB HID report descriptor into a structured report map.

Reads whitespace-separated hex bytes on stdin. Emits, per report ID and
direction, the ordered field list with usage, bit width, count and logical
range -- i.e. exactly what a host needs to decode the report.

Hardware-authoritative: this is the device's own self-description.
"""
import sys
from collections import OrderedDict

MAIN, GLOBAL, LOCAL = 0, 1, 2

MAIN_TAGS = {8: "Input", 9: "Output", 11: "Feature",
             10: "Collection", 12: "EndCollection"}
GLOBAL_TAGS = {0: "UsagePage", 1: "LogicalMin", 2: "LogicalMax",
               3: "PhysicalMin", 4: "PhysicalMax", 5: "UnitExp", 6: "Unit",
               7: "ReportSize", 8: "ReportID", 9: "ReportCount",
               10: "Push", 11: "Pop"}
LOCAL_TAGS = {0: "Usage", 1: "UsageMin", 2: "UsageMax"}

COLLECTION = {0: "Physical", 1: "Application", 2: "Logical", 3: "Report"}


def parse(data):
    i = 0
    g = {"UsagePage": None, "LogicalMin": 0, "LogicalMax": 0,
         "ReportSize": 0, "ReportCount": 0, "ReportID": None}
    local_usages = []
    depth = 0
    # reports[(report_id, direction)] = list of field dicts
    reports = OrderedDict()
    items = 0

    while i < len(data):
        prefix = data[i]
        size = prefix & 0x03
        size = 4 if size == 3 else size
        itype = (prefix >> 2) & 0x03
        tag = (prefix >> 4) & 0x0F
        val = 0
        for k in range(size):
            val |= data[i + 1 + k] << (8 * k)
        i += 1 + size
        items += 1

        if itype == GLOBAL:
            name = GLOBAL_TAGS.get(tag)
            if name == "Push" or name == "Pop":
                continue
            if name:
                g[name] = val
        elif itype == LOCAL:
            if LOCAL_TAGS.get(tag) == "Usage":
                local_usages.append(val)
        elif itype == MAIN:
            name = MAIN_TAGS.get(tag, f"Main{tag}")
            if name == "Collection":
                depth += 1
            elif name == "EndCollection":
                depth -= 1
            elif name in ("Input", "Output", "Feature"):
                key = (g["ReportID"], name)
                reports.setdefault(key, [])
                reports[key].append({
                    "usage_page": g["UsagePage"],
                    "usages": list(local_usages),
                    "bits": g["ReportSize"],
                    "count": g["ReportCount"],
                    "lmin": g["LogicalMin"],
                    "lmax": g["LogicalMax"],
                    "flags": val,
                    "depth": depth,
                })
            local_usages = []
    return reports, items


def fmt_flags(f):
    bits = []
    bits.append("Const" if f & 0x01 else "Data")
    bits.append("Var" if f & 0x02 else "Array")
    bits.append("Rel" if f & 0x04 else "Abs")
    if f & 0x80:
        bits.append("Buffered")
    return ",".join(bits)


def main():
    raw = sys.stdin.read().split()
    data = [int(b, 16) for b in raw if len(b) == 2]
    reports, items = parse(data)
    print(f"descriptor: {len(data)} bytes, {items} HID items\n")

    by_dir = {"Input": [], "Output": [], "Feature": []}
    for (rid, direction), fields in reports.items():
        by_dir[direction].append((rid, fields))

    for direction in ("Input", "Output", "Feature"):
        entries = by_dir[direction]
        if not entries:
            continue
        print(f"{'='*72}\n{direction.upper()} REPORTS ({len(entries)})\n{'='*72}")
        for rid, fields in entries:
            total_bits = sum(f["bits"] * f["count"] for f in fields)
            total_bytes = (total_bits + 7) // 8
            rid_s = f"0x{rid:02X}" if rid is not None else "(none)"
            print(f"\n  report {rid_s}: {total_bits} bits = {total_bytes} bytes payload"
                  f"  (+1 id = {total_bytes+1})")
            for f in fields:
                us = ",".join(f"0x{u:02X}" for u in f["usages"]) or "-"
                page = f"0x{f['usage_page']:04X}" if f["usage_page"] is not None else "?"
                span = f["bits"] * f["count"]
                print(f"      usage {us:<10} page {page}  "
                      f"{f['count']:>4} x {f['bits']:>3} bits = {span:>5}  "
                      f"range {f['lmin']}..{f['lmax']:<6} [{fmt_flags(f['flags'])}]")


if __name__ == "__main__":
    main()
