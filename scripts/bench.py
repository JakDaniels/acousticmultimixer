#!/usr/bin/env python3
# Copyright 2026 Simon Annetts/Jak Daniels
# SPDX-License-Identifier: MPL-2.0
"""CPU cost of mixing a room: acousticmultimixer against SpatialAudioServer's `matrix` engine.

The `matrix` engine's mixing, as SpatialAudioServer builds it: every voice into one audiointerleave, a tee, then per
listener a queue and an audiomixmatrix (2 × N gains). Ours: one acousticmultimixer, a queue per output. Neither has
the Opus encoders or WebRTC around it, which cost the same either way. Runs as fast as possible (not live) through
Python's GStreamer bindings and reports CPU seconds (user + system) per second of audio, i.e. the share of one
core. --live runs in real time with live sources, as a server does (slower to run, but without the thread
hand-offs of running flat out, which inflate the aggregators' cost).

    cargo build --release
    scripts/bench.py [--participants 32] [--seconds 60] [--live]
"""

import argparse
import json
import math
import os
import resource
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def positions(n):
    """Participants on a 5 m grid, facing different ways, so the gains and pans all differ."""
    side = math.ceil(math.sqrt(n))
    return [((i % side) * 5.0, (i // side) * 5.0, (i * 37) % 360) for i in range(n)]


def gains(n, pos, listener):
    """The stereo gains (2 × n) the matrix engine would set for one listener: outdoor model, SpatialAudioServer pan."""
    lx, ly, lyaw = pos[listener]
    rows = [[0.0] * n, [0.0] * n]
    for s in range(n):
        if s == listener:
            continue
        dx, dy = pos[s][0] - lx, pos[s][1] - ly
        d = math.hypot(dx, dy)
        if d > 60:
            continue
        g = 1.0 if d <= 2 else 2 / d
        c, si = math.cos(math.radians(lyaw)), math.sin(math.radians(lyaw))
        fwd, left = c * dx + si * dy, -si * dx + c * dy
        h = math.hypot(fwd, left)
        pan = 0.0 if h < 1e-6 else -left / h
        a = (pan + 1) * math.pi / 4
        rows[0][s] = g * min(1, math.sqrt(2) * math.cos(a))
        rows[1][s] = g * min(1, math.sqrt(2) * math.sin(a))
    return rows


LIVE = False
ELEMENT_PROPS = ""


def source(i, talking, quiet_gap, buffers):
    common = f"audiotestsrc num-buffers={buffers} samplesperbuffer=480 is-live={str(LIVE).lower()}"
    if i < talking:
        return f"{common} freq={200 + 50 * i}"
    # Not talking: GAP buffers (skipped by an aggregator), or very quiet sound (as decoded comfort noise).
    return f"{common} wave=silence" if quiet_gap else f"{common} volume=0.0001"


CAPS = "audio/x-raw,format=F32LE,rate=48000,channels=1"


# Each builder returns (pipeline description, [matrix per listener]). The child process adds a
# tee → queue → audiomixmatrix → fakesink branch per matrix, setting it before linking (in manual mode
# audiomixmatrix cannot link without one) with gst_util_set_object_arg, as SpatialAudioServer does
# (gst-launch cannot parse its nested arrays).


def ours(n, talking, quiet_gap, buffers):
    pos = positions(n)
    pads = " ".join(f"sink_{i}::x={x} sink_{i}::y={y} sink_{i}::yaw={yaw}" for i, (x, y, yaw) in enumerate(pos))
    parts = [f"acousticmultimixer name=m max-participants={n} {ELEMENT_PROPS} {pads}"]
    parts += [f"{source(i, talking, quiet_gap, buffers)} ! {CAPS} ! m.sink_{i}" for i in range(n)]
    parts += [f"m.mix_{i} ! queue ! fakesink" for i in range(n)]
    return " ".join(parts), []


def matrix(n, talking, quiet_gap, buffers):
    pos = positions(n)
    parts = ["audiointerleave name=i channel-positions-from-input=false ! tee name=t"]
    parts += [f"{source(i, talking, quiet_gap, buffers)} ! {CAPS} ! i.sink_{i}" for i in range(n)]
    matrices = []
    for listener in range(n):
        rows = gains(n, pos, listener)
        # SpatialAudioServer uses 1e-9, never 0, for "off" (audiomixmatrix's sparse mode, PLAN.md section 2a).
        m = ", ".join("<" + ", ".join(f"(double){v if v else 1e-9:.17g}" for v in row) + ">" for row in rows)
        matrices.append(f"<{m}>")
    return " ".join(parts), matrices


def sources_only(n, talking, quiet_gap, buffers):
    """The test sources alone, to subtract: what the mixing itself costs is the rest."""
    return " ".join(f"{source(i, talking, quiet_gap, buffers)} ! {CAPS} ! fakesink" for i in range(n)), []


def child():
    """Runs one pipeline (JSON on stdin) to the end, in this process."""
    import gi

    gi.require_version("Gst", "1.0")
    from gi.repository import Gst

    Gst.init(None)
    description, matrices = json.load(sys.stdin)
    pipeline = Gst.parse_launch(description)
    tee = pipeline.get_by_name("t")
    for value in matrices:
        queue = Gst.ElementFactory.make("queue")
        mx = Gst.ElementFactory.make("audiomixmatrix")
        mx.set_property("in-channels", value.count("(double)") // 2)
        mx.set_property("out-channels", 2)
        mx.set_property("channel-mask", 0x3)
        Gst.util_set_object_arg(mx, "matrix", value)
        sink = Gst.ElementFactory.make("fakesink")
        for e in (queue, mx, sink):
            pipeline.add(e)
        assert tee.link(queue) and queue.link(mx) and mx.link(sink)
    pipeline.set_state(Gst.State.PLAYING)
    msg = pipeline.get_bus().timed_pop_filtered(Gst.CLOCK_TIME_NONE, Gst.MessageType.EOS | Gst.MessageType.ERROR)
    pipeline.set_state(Gst.State.NULL)
    if msg.type == Gst.MessageType.ERROR:
        sys.exit(f"error: {msg.parse_error()}")


def run(built, env):
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    start = time.monotonic()
    subprocess.run([sys.executable, __file__, "--child"], input=json.dumps(built), text=True, env=env, check=True)
    wall = time.monotonic() - start
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    cpu = (after.ru_utime - before.ru_utime) + (after.ru_stime - before.ru_stime)
    return cpu, wall


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--participants", type=int, default=32)
    ap.add_argument("--seconds", type=int, default=60)
    ap.add_argument("--plugin-dir", default=os.path.join(ROOT, "target", "release"))
    ap.add_argument("--live", action="store_true", help="live sources, in real time")
    ap.add_argument("--props", default="", help='acousticmultimixer properties, e.g. "direction-model=pan"')
    ap.add_argument("--ours-only", action="store_true", help="skip the matrix pipeline")
    ap.add_argument("--child", action="store_true", help=argparse.SUPPRESS)
    args = ap.parse_args()
    global LIVE, ELEMENT_PROPS
    LIVE = args.live
    ELEMENT_PROPS = args.props
    if args.child:
        return child()

    env = dict(os.environ, GST_PLUGIN_PATH=args.plugin_dir)
    n, buffers = args.participants, args.seconds * 100
    cases = [
        (f"{n} talking", n, False),
        (f"3 talking, {n - 3} quiet", 3, False),
        (f"3 talking, {n - 3} silent (GAP)", 3, True),
    ]
    mode = "live" if args.live else "as fast as possible"
    props = f", acousticmultimixer {args.props}" if args.props else ""
    print(f"{n} participants, {args.seconds} s of audio, {mode}{props}; CPU % of one core (user + system), with the test")
    print("sources' own cost (sources alone) subtracted\n")
    print("| case | sources alone | acousticmultimixer | matrix (audiointerleave + audiomixmatrix) |")
    print("|---|---|---|---|")
    for name, talking, gap in cases:
        base, _ = run(sources_only(n, talking, gap, buffers), env)
        results = []
        for build in (ours,) if args.ours_only else (ours, matrix):
            cpu, _ = run(build(n, talking, gap, buffers), env)
            results.append(f"{100 * (cpu - base) / args.seconds:.1f} %")
        print(f"| {name} | {100 * base / args.seconds:.1f} % | {' | '.join(results)} |", flush=True)


if __name__ == "__main__":
    sys.exit(main())
