#!/usr/bin/env python3
"""Summarizes a shadow comparison (`guest-init --binder-shadow SERVICES
--binder-shadow-log FILE`, docs/m4-packagemanager.md, slice A): per
method of each shadowed service, the calls and how each ended:

- matched: the model's reply decoded equal to the original's;
- differed: it did not (both replies are in the log line);
- not modelled: the model has no answer for the method yet;
- oneway: a one-way call, answered by the model, nothing to compare;
- failed: the original sent no reply (the target died, the call failed);
- undecodable: a reply could not be decoded as the method's reply;
- incomplete: a ParceledListSlice's fetch failed, so the list could not
  be stitched.

Then the watched nodes and drops (copies the comparison could not keep
up with), and the first differences of each method.

Method names come from the image's AIDL stubs, as in
binder-trace-report.py (--image). With --trace (a `--binder-trace` file
of the same boot), each method's row also shows the trace's count of
calls to its interface, which must equal the shadow's.

Usage: binder-shadow-report.py [--image ROOT] [--dexdump PATH]
           [--trace TRACE] [--differences N] LOG
"""

import argparse
import collections
import importlib.util
import json
import os

OUTCOMES = ["matched", "differed", "not_modelled", "oneway", "failed", "undecodable",
            "incomplete"]


def trace_report():
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "binder-trace-report.py")
    spec = importlib.util.spec_from_file_location("binder_trace_report", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def trace_counts(path, descriptors):
    """(descriptor, code) -> calls in a binder trace."""
    counts = collections.Counter()
    for line in open(path, errors="replace"):
        f = line.rstrip("\n").split("\t")
        if len(f) == 15 and f[5] in descriptors:
            counts[(f[5], int(f[6]))] += 1
    return counts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--image")
    ap.add_argument("--dexdump")
    ap.add_argument("--trace")
    ap.add_argument("--differences", type=int, default=3,
                    help="differences shown per method (default 3)")
    ap.add_argument("log")
    args = ap.parse_args()

    rows = collections.defaultdict(collections.Counter)  # (service, descriptor, code)
    differences = collections.defaultdict(list)
    events = []
    dropped = 0
    for line in open(args.log, errors="replace"):
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if "event" in entry:
            if entry["event"] == "dropped":
                dropped = entry["total"]
            else:
                events.append(entry)
            continue
        key = (entry["service"], entry["descriptor"], entry["code"])
        rows[key][entry["outcome"]] += 1
        if entry["outcome"] in ("differed", "undecodable", "incomplete"):
            differences[key].append(entry)

    methods = {}
    special = {}
    if args.image:
        report = trace_report()
        methods = report.stub_methods(args.image, args.dexdump or report.default_dexdump())
        special = report.SPECIAL

    def method(descriptor, code):
        if code in special:
            return special[code]
        return methods.get(descriptor, {}).get(code) or f"#{code}"

    traced = None
    if args.trace:
        traced = trace_counts(args.trace, {d for _, d, _ in rows if d})

    total = collections.Counter()
    for counts in rows.values():
        total.update(counts)
    calls = sum(total.values())
    print(f"{calls} calls: " + ", ".join(f"{total[o]} {o.replace('_', ' ')}"
                                         for o in OUTCOMES if total[o]))
    for e in events:
        print(f"{e['event']}: {e['service']}" + (f" (node {e['node']})" if "node" in e else ""))
    if dropped:
        print(f"dropped: {dropped} copies (not compared)")
    print()
    header = ["service", "interface", "method", "calls"]
    header += [o.replace("_", " ") for o in OUTCOMES]
    if traced is not None:
        header.append("traced")
    print("| " + " | ".join(header) + " |")
    print("|" + " --- |" * 3 + " ---: |" * (len(header) - 3))
    for key in sorted(rows, key=lambda k: (k[0], -sum(rows[k].values()))):
        service, descriptor, code = key
        counts = rows[key]
        cells = [service, descriptor.rsplit(".", 1)[-1] or "-", method(descriptor, code),
                 str(sum(counts.values()))]
        cells += [str(counts[o]) if counts[o] else "" for o in OUTCOMES]
        if traced is not None:
            cells.append(str(traced[(descriptor, code)]))
        print("| " + " | ".join(cells) + " |")
    if traced is not None:
        missing = [(d, c) for (d, c) in traced if not any(k[1:] == (d, c) for k in rows)]
        for d, c in missing:
            print(f"| - | {d.rsplit('.', 1)[-1]} | {method(d, c)} | 0 | "
                  + " | " * len(OUTCOMES) + f"{traced[(d, c)]} |")

    for key in sorted(differences):
        service, descriptor, code = key
        shown = differences[key][: args.differences]
        print()
        print(f"## {service} {method(descriptor, code)}: {len(differences[key])} "
              f"(first {len(shown)})")
        for entry in shown:
            print()
            print(f"- seq {entry['seq']}, {entry['outcome']}, from pid {entry['from_pid']} "
                  f"uid {entry['from_euid']}")
            for side in ("original", "model"):
                if side in entry:
                    print(f"  - {side}: {json.dumps(entry[side])}")
            if entry.get("identities"):
                print(f"  - identities: {json.dumps(entry['identities'])}")


if __name__ == "__main__":
    main()
