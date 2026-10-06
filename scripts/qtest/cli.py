"""Command line of qtest. Run `./qtest help` for the guide."""
import argparse
import json
import re
import subprocess
import sys
import tarfile

import catalog
import runner  # Adds scripts/e2e to the import path for fixture.
import fixture  # noqa: E402
from runner import EXIT_FAILED, EXIT_MISSING, EXIT_PASSED, EXIT_USAGE, ROOT

GUIDE = ROOT / "docs/testing.md"

DESCRIPTION = """\
Run Qrow's checks and tests: formatting, lint, unit, UI, coverage,
performance, dependency policy, scripts, and end-to-end suites.

Everyday use:
  ./qtest                  fast local checks (the default group)
  ./qtest run ui           one suite
  ./qtest run ui/queries   tests of one suite whose names contain "queries"
  ./qtest run --changed    the suites that your changed files affect
  ./qtest list             suites, groups, and what each one needs
  ./qtest doctor           missing tools, with the fix for each one
  ./qtest fixture up       keep the test servers running between runs
  ./qtest compare main     compare performance with another revision
  ./qtest help TOPIC       a section of docs/testing.md

Exit codes: 0 passed, 1 failed, 2 usage error, 3 missing prerequisite.
"""


def slug(heading):
    return re.sub(r"[^a-z0-9]+", "-", heading.lower()).strip("-")


def guide_sections():
    """Level-two sections of docs/testing.md, keyed by slug."""
    sections = {}
    current = None
    for line in GUIDE.read_text().splitlines():
        if line.startswith("## "):
            current = slug(line[3:])
            sections[current] = [line]
        elif current:
            sections[current].append(line)
    return {key: "\n".join(lines).strip() + "\n" for key, lines in sections.items()}


# Commands -----------------------------------------------------------------


def command_list(args):
    if args.tests:
        return list_tests(args)
    suites = [{
        "name": suite.name,
        "summary": suite.summary,
        "requires": list(suite.requires),
        "macos_only": suite.macos_only,
        "explicit_only": suite.explicit_only,
        "servers": suite.fixture is not None,
        "filterable": suite.filterable,
    } for suite in catalog.SUITES.values()]
    groups = [{"name": name, "summary": summary, "suites": list(members)}
              for name, (summary, members) in catalog.GROUPS.items()]
    if args.json:
        print(json.dumps({"suites": suites, "groups": groups}, indent=2))
        return EXIT_PASSED
    width = max(len(suite["name"]) for suite in suites) + 2
    print("Suites (select by name; `name/filter` selects tests by name where marked *):")
    for suite in suites:
        flags = "*" if suite["filterable"] else " "
        extra = []
        if suite["macos_only"]:
            extra.append("macOS")
        if suite["servers"]:
            extra.append("servers")
        if suite["explicit_only"]:
            extra.append("not in groups")
        tail = f"  [{', '.join(extra)}]" if extra else ""
        print(f"  {suite['name']:<{width}}{flags} {suite['summary']}{tail}")
    print("\nGroups:")
    for group in groups:
        print(f"  {group['name']:<{width}}  {group['summary']}")
        print(f"  {'':<{width}}  {' '.join(group['suites'])}")
    print("\nRun `./qtest list SUITE --tests` to list the tests of a suite.")
    return EXIT_PASSED


def list_tests(args):
    selected, _ = runner.resolve(args.selectors or ["default"])
    tests = []
    for item in selected:
        for step in item.suite.steps:
            if step.nextest_filter is None:
                continue
            command = runner.render_command(step, {}, item.test_filter, [])
            command[2] = "list"
            command += ["--message-format", "json"]
            result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
            if result.returncode:
                sys.stderr.write(result.stderr)
                return EXIT_FAILED
            listing = json.loads(result.stdout)
            for binary in listing["rust-suites"].values():
                for name, case in binary["testcases"].items():
                    if case["filter-match"]["status"] == "matches":
                        tests.append({"suite": item.suite.name, "binary": binary["binary-id"], "test": name})
    if args.json:
        print(json.dumps(tests, indent=2))
    else:
        for test in tests:
            print(f"{test['suite']}  {test['binary']}  {test['test']}")
    return EXIT_PASSED


def command_run(args):
    if args.repeat < 1:
        raise runner.UsageError("--repeat must be 1 or more.")
    for selector in args.selectors:
        _, _, test_filter = selector.partition("/")
        if test_filter and not re.fullmatch(r"[A-Za-z0-9_:]+", test_filter):
            raise runner.UsageError(f"Test filters use letters, digits, `_`, and `::`: {test_filter}")
    changed = runner.changed_paths_from_git() if args.changed else None
    selected, notes = runner.resolve(args.selectors, changed)
    output = runner.Output(quiet=args.json or args.quiet)
    for note in notes:
        output.note(note)
    options = {"runtime": args.runtime, "repeat": args.repeat}
    summary = runner.run(selected, options, args.extra, output, fail_fast=args.fail_fast)
    if args.json:
        print(json.dumps(summary, indent=2))
    else:
        print_summary(summary)
    return summary["exit_code"]


def command_hook(args):
    """Run the suites of a Git hook. The hook gives the changed paths in QROW_CHANGED_FILES."""
    names = catalog.suites_for_hook(args.name, runner.changed_paths_from_git())
    output = runner.Output(quiet=False)
    output.note(f"{args.name}: changed paths select " + (", ".join(names) if names else "no suites"))
    selected, _ = runner.resolve(names) if names else ([], [])
    summary = runner.run(selected, {"runtime": "auto", "repeat": 1}, [], output, fail_fast=True)
    print_summary(summary)
    return summary["exit_code"]


def command_ci(args):
    """List the CI jobs, print the jobs for the changed paths, or run the suites of one job."""
    if args.job is None:
        for job in catalog.CI_JOBS.values():
            suites = ", ".join([*job.suites, *(f"{name} (report only)" for name in job.report_only)])
            needs = f"  waits for {', '.join(job.needs)}" if job.needs else ""
            print(f"{job.name:<8} {job.runner:<13} {suites}{needs}")
        return EXIT_PASSED
    if args.job == "plan":
        names = list(catalog.CI_JOBS) if args.all else catalog.ci_jobs_for_changes(runner.changed_paths_from_git())
        plan = json.dumps({name: name in names for name in catalog.CI_JOBS})
        print(plan)
        if args.github_output:
            with open(args.github_output, "a") as output:
                output.write(f"jobs={plan}\n")
        return EXIT_PASSED
    job = catalog.CI_JOBS[args.job]
    names = [*job.suites, *job.report_only]
    if args.install:
        tools = [tool for tool in catalog.TOOLS if any(tool in catalog.SUITES[name].requires for name in names)]
        if install_tools(tools):
            return EXIT_FAILED
    selected, _ = runner.resolve(names)
    output = runner.Output(quiet=False)
    output.note(f"CI job {job.name}: {', '.join(names)}")
    # A CI job stops at once when a prerequisite is missing, for example the
    # automation permissions of the desktop suite, so it builds nothing.
    summary = runner.run(selected, {"runtime": job.runtime, "repeat": 1}, [], output,
                         report_only=job.report_only, require_all=True)
    print_summary(summary)
    return summary["exit_code"]


def print_summary(summary):
    print(f"\nqtest run {summary['run']}: {summary['status']}", file=sys.stderr)
    for suite in summary["suites"]:
        duration = f"{suite['duration_s']:.0f}s" if suite.get("duration_s") else ""
        note = " (report only)" if suite.get("report_only") else ""
        print(f"  {suite['status']:<8} {suite['name']:<10} {duration:>6}  {suite.get('reason', '')}{note}",
              file=sys.stderr)
        for failure in suite.get("failures", []):
            print(f"           FAILED {failure['test']}", file=sys.stderr)
        if suite["status"] == "failed" and suite.get("log"):
            print(f"           log: {suite['log']}", file=sys.stderr)
    print(f"  artifacts: {summary['artifacts']}", file=sys.stderr)


def command_doctor(args):
    if args.selectors:
        selected, _ = runner.resolve(args.selectors)
        suites = [item.suite for item in selected]
    else:
        suites = [suite for suite in catalog.SUITES.values() if catalog.MACOS or not suite.macos_only]
    options = {"runtime": args.runtime}
    results = {}
    for suite in suites:
        for requirement in suite.requires:
            if requirement not in results:
                results[requirement] = runner.check_requirement(requirement, options)
    report = [{"requirement": name, "ok": fix is None, "fix": fix,
               "suites": [suite.name for suite in suites if name in suite.requires]}
              for name, fix in results.items()]
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        for entry in report:
            mark = "ok     " if entry["ok"] else "MISSING"
            print(f"{mark} {entry['requirement']:<16} ({', '.join(entry['suites'])})")
            if entry["fix"]:
                print(f"        {entry['fix']}")
    return EXIT_PASSED if all(entry["ok"] for entry in report) else EXIT_MISSING


def command_install(args):
    names = args.tools or list(catalog.TOOLS)
    unknown = [name for name in names if name not in catalog.TOOLS]
    if unknown:
        raise runner.UsageError(f"Unknown tool: {', '.join(unknown)}. Known: {', '.join(catalog.TOOLS)}")
    return install_tools(names)


def install_archive(tool, archive):
    """Download a prebuilt tool archive, check its digest, and unpack the tool into the Cargo bin directory."""
    import hashlib
    import os
    import shutil
    import tempfile
    import urllib.request
    from pathlib import Path
    bin_dir = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")) / "bin"
    print(f"[qtest] Downloading {archive.url}", file=sys.stderr)
    with tempfile.TemporaryDirectory() as directory:
        bundle_path = Path(directory) / "tool.tar.gz"
        with urllib.request.urlopen(archive.url, timeout=120) as response:
            bundle_path.write_bytes(response.read())
        actual = hashlib.sha256(bundle_path.read_bytes()).hexdigest()
        if actual != archive.sha256:
            print(f"[qtest] {tool.name}: SHA-256 {actual} does not match the pinned {archive.sha256}.",
                  file=sys.stderr)
            return False
        unpacked = Path(directory) / "unpacked"
        with tarfile.open(bundle_path) as bundle:
            bundle.extract(bundle.getmember(archive.member), unpacked, filter="data")
        bin_dir.mkdir(parents=True, exist_ok=True)
        shutil.move(unpacked / archive.member, bin_dir / tool.name)
        (bin_dir / tool.name).chmod(0o755)
    return True


def install_tools(names):
    for name in names:
        tool = catalog.TOOLS[name]
        if runner.check_requirement(name, {}) is None:
            print(f"[qtest] {name} {tool.version} is installed.", file=sys.stderr)
            continue
        archive = next((archive for archive in tool.archives if archive.platform == catalog.host_platform()), None)
        installed = False
        if archive:
            try:
                installed = install_archive(tool, archive) and runner.check_requirement(name, {}) is None
            except (OSError, KeyError, tarfile.TarError) as error:
                print(f"[qtest] {name}: the archive did not install ({error}).", file=sys.stderr)
            if not installed:
                print(f"[qtest] {name}: building it from source instead.", file=sys.stderr)
        for command in (() if installed else tool.install) + tool.setup:
            print(f"[qtest] $ {subprocess.list2cmdline(command)}", file=sys.stderr)
            if subprocess.run(command, cwd=ROOT).returncode:
                return EXIT_FAILED
    return EXIT_PASSED


def command_help(args):
    sections = guide_sections()
    if not args.topic:
        print(DESCRIPTION)
        print("Commands: " + ", ".join(COMMANDS))
        print("Run `./qtest COMMAND --help` for the options of a command.\n")
        print("Topics (sections of docs/testing.md):")
        for key in sections:
            print(f"  {key}")
        return EXIT_PASSED
    if args.topic not in sections:
        raise runner.UsageError(f"Unknown topic: {args.topic}. Run `./qtest help` for the list.")
    print(sections[args.topic], end="")
    return EXIT_PASSED


UI_TEST_TEMPLATE = '''
#[gpui_kit::test]
fn {name}(cx: &mut TestAppContext) {{
    let app = TestApp::launch(cx, Workspace::default());
    // Operate the window through element IDs, then check the visible result
    // and the saved workspace. See `./qtest help write-a-ui-test`.
    let _ = &app;
    panic!("Write the test {name}");
}}
'''

UI_SUITE_HEADER = '''use crate::support::TestApp;
use gpui_kit::TestAppContext;
use qrow::model::Workspace;
'''

E2E_TEST_TEMPLATE = '''
#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn {name}(cx: &mut TestAppContext) {{
    let kyuubi = Kyuubi::get();
    let (workspace, credentials) = kyuubi.workspace("SELECT 1 AS value", PASSWORD);
    let app = TestApp::launch_with(cx, workspace, credentials);
    // Run SQL through the window, wait with QUERY_TIMEOUT, and check the
    // result table. See `./qtest help write-an-e2e-test`.
    let _ = &app;
    panic!("Write the test {name}");
}}
'''

E2E_SUITE_HEADER = '''use crate::support::TestApp;
use crate::support::fixture::{Kyuubi, PASSWORD};
use gpui_kit::TestAppContext;
'''

TEMPLATES = {"ui": (UI_SUITE_HEADER, UI_TEST_TEMPLATE), "e2e": (E2E_SUITE_HEADER, E2E_TEST_TEMPLATE)}


def command_new(args):
    for value, label in ((args.suite, "Suite"), (args.name, "Test name")):
        if not re.fullmatch(r"[a-z][a-z0-9_]*", value):
            raise runner.UsageError(f"{label} must be snake_case: {value}")
    directory = ROOT / "tests" / args.layer
    main = directory / "main.rs"
    path = directory / f"{args.suite}.rs"
    if args.suite in ("main", "support"):
        raise runner.UsageError(f"{args.suite} is reserved.")
    if path.exists() and re.search(rf"\bfn {args.name}\(", path.read_text()):
        raise runner.UsageError(f"{path.relative_to(ROOT)} already has {args.name}.")
    header, template = TEMPLATES[args.layer]
    text = path.read_text() if path.exists() else header
    path.write_text(text + template.format(name=args.name))
    lines = main.read_text().splitlines()
    module = f"mod {args.suite};"
    if module not in lines:
        main.write_text("\n".join(insert_module(lines, args.suite)) + "\n")
    print(f"Added {args.name} to {path.relative_to(ROOT)}. It fails until you write it.")
    print(f"Run it with: ./qtest run {args.layer}/{args.name}")
    return EXIT_PASSED


def insert_module(lines, name):
    """Add `mod name;` among the plain module declarations in sorted order.

    A declaration with an attribute, like `#[path = …] mod support;`, stays
    with its attribute.
    """
    plain = [index for index, line in enumerate(lines)
             if re.fullmatch(r"mod \w+;", line) and not (index and lines[index - 1].startswith("#["))]
    later = [index for index in plain if lines[index][4:-1] > name]
    if later:
        position = later[0]
    elif plain:
        position = plain[-1] + 1
    else:
        position = len(lines)
    return [*lines[:position], f"mod {name};", *lines[position:]]


def command_compare(args):
    import compare
    if args.rounds < 1:
        raise runner.UsageError("--rounds must be 1 or more.")
    suites = args.suites or list(catalog.COMPARE_SUITES)
    for name in suites:
        if name not in catalog.SUITES:
            raise runner.UsageError(f"Unknown suite: {name}")
    output = runner.Output(quiet=args.json)
    report, code = compare.compare(args.ref, suites, args.rounds, args.threshold, output.note)
    if report is None:
        return code
    print(json.dumps(report, indent=2) if args.json else compare.format_report(report))
    return code


def command_fixture(args):
    argv = [args.action]
    if args.action == "up":
        argv += ["--runtime", args.runtime]
    return fixture.main(argv)


def command_artifacts(args):
    latest = runner.runs_dir() / "latest"
    summary = latest / "summary.json"
    if not summary.exists():
        print("No qtest run has finished yet.", file=sys.stderr)
        return EXIT_FAILED
    data = json.loads(summary.read_text())
    if args.json:
        print(json.dumps(data, indent=2))
    else:
        print(data["artifacts"])
    return EXIT_PASSED


COMMANDS = {
    "list": command_list,
    "run": command_run,
    "hook": command_hook,
    "ci": command_ci,
    "doctor": command_doctor,
    "install": command_install,
    "new": command_new,
    "artifacts": command_artifacts,
    "fixture": command_fixture,
    "compare": command_compare,
    "help": command_help,
}


def parser():
    root = argparse.ArgumentParser(prog="qtest", description=DESCRIPTION,
                                   formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = root.add_subparsers(dest="command")

    listing = commands.add_parser("list", help="Show suites and groups, or the tests of suites.")
    listing.add_argument("selectors", nargs="*", help="Suites or groups whose tests to list (with --tests).")
    listing.add_argument("--tests", action="store_true", help="List test names through cargo-nextest.")
    listing.add_argument("--json", action="store_true", help="Print JSON.")

    running = commands.add_parser(
        "run", help="Run suites, groups, or filtered tests.",
        description="Run suites or groups. SUITE/FILTER runs the tests of SUITE whose names "
                    "contain FILTER. Arguments after `--` go to cargo-nextest.")
    running.add_argument("selectors", nargs="*", metavar="SELECTOR",
                         help="Suite, group, or SUITE/FILTER. Default: the default group.")
    running.add_argument("--changed", action="store_true",
                         help="Add the suites that changed paths affect.")
    running.add_argument("--repeat", type=int, default=1, metavar="N",
                         help="Run each selected suite N times and stop at the first failure.")
    running.add_argument("--fail-fast", action="store_true", help="Skip the remaining suites after a failure.")
    running.add_argument("--runtime", choices=["auto", "docker", "native"], default="auto",
                         help="Server runtime of the backend, e2e, and desktop suites. auto (the "
                              "default) uses Docker when its daemon answers, else local Java processes.")
    running.add_argument("--json", action="store_true", help="Print only a JSON summary on stdout.")
    running.add_argument("--quiet", action="store_true", help="Do not stream command output.")

    hook = commands.add_parser(
        "hook", help="Run the suites of a Git hook for the paths in QROW_CHANGED_FILES.",
        description="The repository hooks call this command in a snapshot of the commit. "
                    "It runs the suites that the changed paths select and that the hook includes.")
    hook.add_argument("name", choices=list(catalog.HOOKS))

    ci = commands.add_parser(
        "ci", help="List the CI jobs, plan them for changed paths, or run one job.",
        description="Without JOB, list the jobs of the checks workflow. `plan` prints the jobs that "
                    "the changed paths select, as JSON. JOB runs the suites of that job, as CI does.")
    ci.add_argument("job", nargs="?", choices=[*catalog.CI_JOBS, "plan"])
    ci.add_argument("--all", action="store_true", help="plan: select every job.")
    ci.add_argument("--github-output", metavar="FILE", help="plan: also write `jobs=JSON` to FILE.")
    ci.add_argument("--install", action="store_true", help="Install the pinned tools of the job first.")

    doctor = commands.add_parser("doctor", help="Check prerequisites and print a fix for each gap.")
    doctor.add_argument("selectors", nargs="*", help="Check only these suites or groups.")
    doctor.add_argument("--runtime", choices=["auto", "docker", "native"], default="auto")
    doctor.add_argument("--json", action="store_true", help="Print JSON.")

    install = commands.add_parser("install", help="Install the pinned Cargo tools.")
    install.add_argument("tools", nargs="*", help=f"Default: {', '.join(catalog.TOOLS)}.")

    new = commands.add_parser("new", help="Add a test from the template.")
    new.add_argument("layer", choices=list(TEMPLATES), help="Test layer.")
    new.add_argument("suite", help="File under tests/LAYER/, for example connections.")
    new.add_argument("name", help="Test function name, in snake_case.")

    servers = commands.add_parser(
        "fixture", help="Start, stop, or check servers that later runs reuse.",
        description="`up` starts the disposable servers and keeps them running. The backend, "
                    "e2e, and desktop suites reuse them, which saves the start time. `down` stops them.")
    servers.add_argument("action", choices=["up", "down", "status"])
    servers.add_argument("--runtime", choices=["auto", "docker", "native"], default="auto",
                         help="auto: Docker when its daemon answers, else local Java processes.")

    comparing = commands.add_parser(
        "compare", help="Compare performance probes with another revision on this machine.",
        description="Build REF in a worktree under target/qtest/compare/, run the performance suites "
                    "there and here in alternating rounds, and report the median change of each probe. "
                    "Exits with 1 when a probe is slower than the threshold.")
    comparing.add_argument("ref", help="A revision that has the performance suites, for example main.")
    comparing.add_argument("suites", nargs="*", help=f"Default: {', '.join(catalog.COMPARE_SUITES)}.")
    comparing.add_argument("--rounds", type=int, default=3, help="Runs of each side (default: 3).")
    comparing.add_argument("--threshold", type=float, default=25.,
                           help="The change in percent above which a probe counts as slower (default: 25).")
    comparing.add_argument("--json", action="store_true", help="Print only a JSON report on stdout.")

    artifacts = commands.add_parser("artifacts", help="Print the directory of the latest run.")
    artifacts.add_argument("--json", action="store_true", help="Print the summary of the latest run.")

    helping = commands.add_parser("help", help="Show the guide or one of its topics.")
    helping.add_argument("topic", nargs="?")
    return root


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    extra = []
    if "--" in argv:
        index = argv.index("--")
        argv, extra = argv[:index], argv[index + 1:]
    if not argv:
        argv = ["run"]
    arguments = parser().parse_args(argv)
    arguments.extra = extra
    if extra and arguments.command != "run":
        print("qtest: arguments after `--` apply only to `run`.", file=sys.stderr)
        return EXIT_USAGE
    try:
        return COMMANDS[arguments.command](arguments)
    except runner.UsageError as error:
        print(f"qtest: {error}", file=sys.stderr)
        return EXIT_USAGE
    except KeyboardInterrupt:
        return 130
