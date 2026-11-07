import sys
import signal

from cli import main


def cancelled(_number, _frame):
    raise KeyboardInterrupt


signal.signal(signal.SIGTERM, cancelled)
sys.exit(main())
