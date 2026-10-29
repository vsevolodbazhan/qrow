"""Compare the performance probes of another revision with the working tree.

Both trees build and run the same suites on this machine, in alternating
order, so that machine noise affects both sides alike. The revision must
contain the performance suites of qtest.
"""
import json
import os
import statistics
import subprocess

from runner import ROOT, EXIT_FAILED, EXIT_MISSING, EXIT_PASSED, UsageError, target_dir


def base_tree(ref):
    """A detached worktree of `ref` under target/qtest/compare/."""
    result = subprocess.run(["git", "rev-parse", "--verify", f"{ref}^{{commit}}"], cwd=ROOT,
                            capture_output=True, text=True)
    if result.returncode:
        raise UsageError(f"Unknown revision: {ref}")
    commit = result.stdout.strip()
    path = target_dir() / "qtest" / "compare" / commit[:12]
    if not path.exists():
        subprocess.run(["git", "worktree", "add", "--detach", str(path), commit], cwd=ROOT,
                       check=True, capture_output=True)
    return path, commit


def run_once(tree, suites, target, output):
    """Run the suites in `tree` and return {probe: (unit, [values])}."""
    command = [str(tree / "qtest"), "run", *suites, "--json", "--quiet"]
    env = dict(os.environ, CARGO_TARGET_DIR=str(target))
    output(f"{tree}: {' '.join(suites)}")
    result = subprocess.run(command, cwd=tree, env=env, capture_output=True, text=True)
    try:
        summary = json.loads(result.stdout)
    except json.JSONDecodeError:
        raise RuntimeError(f"{tree} did not print a qtest summary: {result.stderr[-2000:]}")
    if summary["exit_code"] not in (EXIT_PASSED, EXIT_FAILED):
        raise RuntimeError(f"{tree} could not run {suites}: {summary}")
    measured = {}
    for suite in summary["suites"]:
        for metric in suite.get("metrics", []):
            unit, values = measured.setdefault(metric["probe"], (metric["unit"], []))
            values.append(metric["value"])
    return measured


def merge(total, measured):
    for probe, (unit, values) in measured.items():
        total.setdefault(probe, (unit, []))[1].extend(values)


def summarize(base, head, threshold):
    """One row per probe that both sides measured. Lower values are better."""
    rows = []
    for probe in sorted(set(base) & set(head)):
        unit, base_values = base[probe]
        _, head_values = head[probe]
        before, after = statistics.median(base_values), statistics.median(head_values)
        change = (after - before) / before * 100 if before else 0.0
        rows.append({"probe": probe, "unit": unit, "base": before, "head": after,
                     "change_percent": change, "regression": change > threshold})
    return rows


def compare(ref, suites, rounds, threshold, output):
    tree, commit = base_tree(ref)
    if not (tree / "qtest").exists():
        output(f"{ref} has no ./qtest. Compare needs a revision with the performance suites.")
        return None, EXIT_MISSING
    base_target = target_dir() / "qtest" / "compare" / "target"
    sides = [("base", tree, base_target), ("head", ROOT, target_dir())]
    totals = {"base": {}, "head": {}}
    for round_number in range(rounds):
        # Alternate which side goes first, so slow drift affects both.
        order = sides if round_number % 2 == 0 else list(reversed(sides))
        for name, path, target in order:
            merge(totals[name], run_once(path, suites, target, output))
    rows = summarize(totals["base"], totals["head"], threshold)
    report = {"base": commit, "suites": list(suites), "rounds": rounds, "threshold_percent": threshold,
              "probes": rows}
    code = EXIT_FAILED if any(row["regression"] for row in rows) else EXIT_PASSED
    return report, code


def format_report(report):
    lines = [f"Compared with {report['base'][:12]} over {report['rounds']} rounds "
             f"(regression above +{report['threshold_percent']:g}%):"]
    for row in report["probes"]:
        mark = "SLOWER" if row["regression"] else ""
        lines.append(f"  {row['probe']:<34} {row['base']:>10.2f} -> {row['head']:>10.2f} {row['unit']:<3}"
                     f" {row['change_percent']:+7.1f}%  {mark}")
    return "\n".join(lines)

