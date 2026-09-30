#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
. scripts/core/preflight.sh
qrow_require_commands uv thrift
qrow_preflight_finish || exit 1
thrift --gen rs -out src/connector vendor/TCLIService.thrift
# Thrift 0.24's Rust generator boxes union elements incorrectly and shadows a map.
# Keep these narrowly scoped corrections reproducible alongside the pinned IDL.
# Qrow is only a client, so also remove the server processor. The bindings then
# build without the `server` feature of the thrift crate.
uv run --locked python - <<'PY'
from pathlib import Path
p = Path('src/connector/t_c_l_i_service.rs')
s = p.read_text()
assert s.count('val.push(Box::new(elem))') == 3
assert s.count('Ok(val) => { val.insert(map_key_0, val); }') == 1
s = s.replace('val.push(Box::new(elem))', 'val.push(elem)')
s = s.replace('Ok(val) => { val.insert(map_key_0, val); }', 'Ok(value) => { val.insert(map_key_0, value); }')
server_import = 'use thrift::server::TProcessor;\n'
assert s.count(server_import) == 1
s = s.replace(server_import, '')
processor_start = '//\n// TCLIService service processor\n//\n\n'
processor_impl = 'impl <H: TCLIServiceSyncHandler> TProcessor for TCLIServiceSyncProcessor<H> {\n'
assert s.count(processor_start) == 1 and s.count(processor_impl) == 1
start = s.index(processor_start)
end = s.index('\n//\n// ', s.index(processor_impl)) + 1
s = s[:start] + s[end:]
assert 'TProcessor' not in s and 'thrift::server' not in s and 'TCLIServiceSyncHandler' not in s
s = s.replace('#![cfg_attr(rustfmt, rustfmt_skip)]\n', '')
p.write_text(s.rstrip() + '\n')
PY
