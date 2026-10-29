"""Check typed Parquet values with the reference reader."""

from datetime import date, datetime
from decimal import Decimal
import json
import sys

import duckdb

connection = duckdb.connect()
rows = connection.execute("SELECT * FROM read_parquet(?)", [sys.argv[1]]).fetchall()
assert len(rows) == 1, rows
text, absent, empty, amount, identifier, ok, binary, items, day, moment = rows[0]
assert text == "a|b", text
assert absent is None, absent
assert empty == "", empty
assert amount == Decimal("123.45"), amount
assert identifier == 9223372036854775807, identifier
assert ok is True, ok
assert binary == bytes([0, 92, 255]), binary
assert json.loads(items) == [1, 2], items
assert day == date(2026, 10, 9), day
assert moment == datetime(2026, 10, 9, 1, 2, 3, 123456), moment
