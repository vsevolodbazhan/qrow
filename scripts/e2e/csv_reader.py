"""Check CSV null and empty-string semantics with the reference reader."""

import json
from pathlib import Path
import sys
import tempfile

import duckdb

case = json.load(sys.stdin)
with tempfile.TemporaryDirectory(prefix="qrow-csv-reader-") as directory:
    path = Path(directory) / "result.csv"
    with path.open("w", encoding="utf-8", newline="") as output:
        output.write(case["csv"])
    rows = duckdb.connect().execute(
        "SELECT * FROM read_csv(?, header=true, all_varchar=true, "
        "nullstr=?, allow_quoted_nulls=false)",
        [str(path), case["null"]],
    ).fetchall()
    actual = [list(row) for row in rows]
    assert actual == case["rows"], (actual, case["rows"])
