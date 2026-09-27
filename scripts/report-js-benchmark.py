#!/usr/bin/env python3
"""Display the JS benchmark report written by the full test run."""

import json
import math
import sys
from pathlib import Path


def finite_number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def main(path):
    report = json.loads(Path(path).read_text())
    shapes = report["shapes"]
    if not isinstance(shapes, list) or not shapes or report["total"] != len(shapes):
        raise ValueError("JS benchmark report has missing or incomplete shapes")

    print(
        "JS benchmark: shapes={} runs={} profile={} passes={} target={}-{} "
        "(baseline v{}; reference: {})".format(
            report["total"],
            report["measurement_runs"],
            report["measured_profile"],
            report["measured_passes"],
            report["target_os"],
            report["target_arch"],
            report["baseline_version"],
            report["reference_engine"],
        )
    )
    print("  fixture v{} sha256={} (all pass results validated)".format(
        report["fixture"]["version"], report["fixture"]["sha256"]
    ))
    if not report["baseline_comparable"]:
        print(
            "  note: baseline drift is advisory; current=profile:{} runs:{} "
            "target={}-{} environment={!r}; baseline=profile:{} runs:{} "
            "target={}-{} environment={!r}".format(
                report["measured_profile"], report["measurement_runs"],
                report["target_os"], report["target_arch"], report["environment"],
                report["baseline_profile"], report["baseline_measurement_runs"],
                report["baseline_target_os"], report["baseline_target_arch"],
                report["baseline_environment"],
            )
        )
    diagnostics = report["jit_diagnostics"]
    print(
        "  baseline JIT: enabled={} requests={} compiled={} rejected={} "
        "time={}ns code={}B entries={} bailouts={} property(hit/miss/bailout)={}/{}/{}".format(
            diagnostics["enabled"], diagnostics["compile_requests"],
            diagnostics["successful_compilations"], diagnostics["compile_rejections"],
            diagnostics["total_compile_time_ns"], diagnostics["generated_code_bytes"],
            diagnostics["compiled_entries"], diagnostics["bailouts"],
            diagnostics["property_guard_hits"], diagnostics["property_guard_misses"],
            diagnostics["property_bailouts"],
        )
    )
    print("  {:<26} {:>10} {:>10} {:>8} {:>9} {:>8}  {}".format(
        "shape", "median", "range", "delta", "vs SM-int", "vs SM-jit", "drift"
    ))
    for shape in shapes:
        numbers = [shape[key] for key in (
            "ns_per_op", "min_ns_per_op", "max_ns_per_op", "delta_ratio",
            "versus_interpreter", "versus_jit",
        )]
        if not all(map(finite_number, numbers)) or shape["drift"] not in (
            "improved", "unchanged", "regressed"
        ):
            raise ValueError("JS benchmark report has invalid shape measurements")
        print("  {:<26} {:>10.1f} {:>4.0f}-{:<4.0f} {:>7.0f}% {:>8.1f}x {:>7.0f}x  {}".format(
            shape["id"],
            shape["ns_per_op"],
            shape["min_ns_per_op"],
            shape["max_ns_per_op"],
            shape["delta_ratio"] * 100,
            shape["versus_interpreter"],
            shape["versus_jit"],
            "" if shape["drift"] == "unchanged" else shape["drift"],
        ))
    for key in ("improvements", "regressions"):
        if report[key]:
            print("  {}: {}".format(key, ", ".join(report[key])))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: report-js-benchmark.py REPORT.json")
    try:
        main(sys.argv[1])
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise SystemExit(f"invalid JS benchmark report: {error}") from error
