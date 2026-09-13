#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands python3 thrift
qrow_require_python_311
qrow_preflight_finish || exit 1
thrift --gen rs -out src/connector vendor/TCLIService.thrift
# Thrift 0.24's Rust generator boxes union elements incorrectly and shadows a map.
# Keep these narrowly scoped corrections reproducible alongside the pinned IDL.
python3 - <<'PY'
from pathlib import Path
p = Path('src/connector/t_c_l_i_service.rs')
s = p.read_text()
assert s.count('val.push(Box::new(elem))') == 3
assert s.count('Ok(val) => { val.insert(map_key_0, val); }') == 1
s = s.replace('val.push(Box::new(elem))', 'val.push(elem)')
s = s.replace('Ok(val) => { val.insert(map_key_0, val); }', 'Ok(value) => { val.insert(map_key_0, value); }')
s = s.replace('#![cfg_attr(rustfmt, rustfmt_skip)]\n', '')
p.write_text(s.rstrip() + '\n')
PY
