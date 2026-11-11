#!/usr/bin/env python3
"""Prints a boot's trace (`DATA.run/logs/boot-trace`, written by guest-init;
docs/boot-status.md, "Boot trace"): its critical path to a property set
(`sys.boot_completed=1` by default), or with --timeline every event.

The trace's lines are `START END LANE WHAT ON`, tab-separated, times in
microseconds since the boot's epoch (`ro.boottime.*`'s). The path is
walked back from the set: on a lane, to the latest event that ended
before; from a wait, along its edge (ON): `property:N=V` to the latest set
of N=V (any of `&&`-joined terms), `lane:L` to lane L at the wait's end,
`stage:S` to the latest span named S. Time on a lane between its events
is named after the span that encloses it, else `(untraced)`.

Usage: boot-trace.py [--to NAME=VALUE] [--timeline] DATA|DATA.run|TRACE
"""

import argparse
import bisect
import os
import sys
from collections import defaultdict


class Event:
    def __init__(self, index, start, end, lane, what, on):
        self.index, self.start, self.end = index, start, end
        self.lane, self.what, self.on = lane, what, on

    def set_of(self):
        """(name, value) this event set, if it set a property."""
        what = self.what
        if what.startswith("set ") and "=" in what:
            name, value = what[4:].split("=", 1)
            return name, value
        # init's own `setprop` builtin.
        if self.lane == "init" and "] setprop " in what:
            parts = what.split("] setprop ", 1)[1].split(" ", 1)
            if len(parts) == 2:
                return parts[0], parts[1]
        return None


def parse(text):
    events = []
    for line in text.splitlines():
        fields = line.split("\t")
        if len(fields) != 5:
            continue
        start, end = int(fields[0]), int(fields[1])
        events.append(Event(len(events), start, end, *fields[2:]))
    return events


class Trace:
    def __init__(self, events):
        self.events = events
        self.lanes = defaultdict(list)
        self.sets = defaultdict(list)
        self.named = defaultdict(list)
        for e in events:
            self.lanes[e.lane].append(e)
            self.named[e.what].append(e)
            s = e.set_of()
            if s:
                self.sets[s[0]].append((e.end, s[1], e))
        # Each lane by (end, start, -index), so the latest event to end is
        # last, and its spans by start with the latest end so far.
        self.ends = {}
        self.spans = {}
        for lane, lane_events in self.lanes.items():
            lane_events.sort(key=self.key)
            self.ends[lane] = [e.end for e in lane_events]
            spans = sorted((e for e in lane_events if e.end > e.start), key=lambda e: e.start)
            reach, latest = [], 0
            for e in spans:
                latest = max(latest, e.end)
                reach.append(latest)
            self.spans[lane] = (spans, [e.start for e in spans], reach)

    @staticmethod
    def key(e):
        return (e.end, e.start, -e.index)

    def latest_set(self, terms, before):
        best = None
        for term in terms.split(" && "):
            if "=" not in term:
                continue
            name, value = term.split("=", 1)
            for at, v, e in self.sets.get(name, []):
                if at <= before and (value in ("*", v)) and (best is None or at > best.end):
                    best = e
        return best

    def pick(self, lane, before, used):
        """The latest event of `lane` that ended by `before`, innermost."""
        events = self.lanes.get(lane, [])
        i = bisect.bisect_right(self.ends.get(lane, []), before) - 1
        while i >= 0 and events[i].index in used:
            i -= 1
        if i < 0:
            # Else the span the walk is in (a service started by a command).
            within = [e for e in events if e.start < before < e.end and e.index not in used]
            return min(within, key=lambda e: e.end - e.start, default=None)
        best = events[i]
        # Down to the latest event to end within it, while there is one.
        j = i - 1
        while j >= 0 and events[j].end > best.start:
            e = events[j]
            if (e.index not in used and e.start >= best.start
                    and e.end - e.start < best.end - best.start):
                best = e
            j -= 1
        return best

    def enclosing(self, lane, at):
        """The innermost span of `lane` around `at`."""
        spans, starts, reach = self.spans.get(lane, ([], [], []))
        j = bisect.bisect_right(starts, at) - 1
        while j >= 0 and reach[j] > at:
            if spans[j].end > at:
                return spans[j].what
            j -= 1
        return "(running)" if lane.startswith("svc/") else "(untraced)"

    def edge(self, e):
        """Where a walk goes from `e`: (lane, before), or None to go on.
        A wait goes to its other end only if that came after it began."""
        if e.on.startswith("property:"):
            s = self.latest_set(e.on[len("property:"):], e.end)
            if s is not None and s is not e and s.end >= e.start:
                return s.lane, s.end
        elif e.on.startswith("lane:"):
            return e.on[len("lane:"):], e.end
        elif e.on.startswith("stage:"):
            name = e.on[len("stage:"):]
            runs = [x for x in self.named.get(name, []) if e.start <= x.end <= e.end]
            if runs:
                run = max(runs, key=lambda x: x.end)
                return run.lane, run.end
        return None

    def critical_path(self, target):
        """(start, end, lane, label) steps from the boot to `target`."""
        steps = [(target.start, target.end, target.lane, target.what)]
        used = {target.index}
        lane, before = target.lane, target.start
        while True:
            e = self.pick(lane, before, used)
            if e is None:
                break
            used.add(e.index)
            if before > e.end:
                label = self.enclosing(lane, (e.end + before) // 2)
                steps.append((e.end, before, lane, label))
            label = e.what if not e.on else f"{e.what} on {e.on}"
            end = min(e.end, before)
            jump = self.edge(e)
            if jump is None:
                steps.append((e.start, end, lane, label))
                before = e.start
            else:
                # The wait's time until its other end is that end's lane's.
                lane, before = jump[0], min(jump[1], end)
                steps.append((max(e.start, before), end, e.lane, label))
        steps.reverse()
        if steps[0][0] > 0:
            steps.insert(0, (0, steps[0][0], "", "(before the trace's first event on the path)"))
        return steps


# Steps shorter than this (in microseconds) are summed per run.
SHORT = 5000


def ms(us):
    return f"{us / 1000:9.1f}"


def print_path(trace, target, to):
    steps = trace.critical_path(target)
    print(f"critical path to {to} at {target.end / 1e6:.3f} s ({target.lane})")
    print(f"{'start ms':>9} {'took ms':>9}  lane: what")
    by_lane = defaultdict(int)
    # Runs of short steps on one lane are a line each.
    lines = []
    for start, end, lane, label in steps:
        by_lane[lane] += end - start
        short = end - start < SHORT
        if lines and short and lines[-1][4] and lines[-1][2] == lane:
            lines[-1][1], lines[-1][3] = end, lines[-1][3] + 1
        else:
            lines.append([start, end, lane, 1, short, label])
    for start, end, lane, count, short, label in lines:
        if count > 1:
            label = f"{count} steps from {label}"
        elif short and end - start < 1000:
            continue
        print(f"{ms(start)} {ms(end - start)}  {lane}: {label}")
    print("by lane (ms):")
    for lane, us in sorted(by_lane.items(), key=lambda item: -item[1]):
        print(f"{ms(us)}  {lane}")


def print_timeline(trace):
    for e in sorted(trace.events, key=lambda e: (e.start, e.index)):
        on = f"  on {e.on}" if e.on else ""
        print(f"{ms(e.start)} {ms(e.end - e.start)}  {e.lane}: {e.what}{on}")


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--to", default="sys.boot_completed=1")
    parser.add_argument("--timeline", action="store_true")
    parser.add_argument("path")
    args = parser.parse_args(argv)
    path = args.path.rstrip("/")
    if os.path.isdir(path):
        # DATA, or its runtime directory DATA.run.
        run = path if path.endswith(".run") else path + ".run"
        path = os.path.join(run, "logs", "boot-trace")
    try:
        with open(path) as f:
            trace = Trace(parse(f.read()))
    except OSError as error:
        print(f"boot-trace: {error}", file=sys.stderr)
        return 1
    if args.timeline:
        print_timeline(trace)
        return 0
    target = trace.latest_set(args.to, float("inf"))
    if target is None:
        print(f"{path}: no set of {args.to}", file=sys.stderr)
        return 1
    print_path(trace, target, args.to)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
