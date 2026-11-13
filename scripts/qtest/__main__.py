import sys
import signal

from cli import main

# Test output contains characters outside the Windows console code page.
for stream in (sys.stdout, sys.stderr):
    stream.reconfigure(encoding="utf-8", errors="replace")


def cancelled(_number, _frame):
    raise KeyboardInterrupt


signal.signal(signal.SIGTERM, cancelled)
sys.exit(main())
