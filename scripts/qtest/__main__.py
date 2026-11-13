import os
import sys
import signal

from cli import main
from runner import build_cache_environment

# Set before any command, so a plan and its execution see the same build environment.
os.environ.update(build_cache_environment(os.environ))

# Test output contains characters outside the Windows console code page.
for stream in (sys.stdout, sys.stderr):
    stream.reconfigure(encoding="utf-8", errors="replace")


def cancelled(_number, _frame):
    raise KeyboardInterrupt


signal.signal(signal.SIGTERM, cancelled)
sys.exit(main())
