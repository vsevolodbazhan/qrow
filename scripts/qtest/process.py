"""Process-tree ownership and machine-wide advisory leases for test resources."""
import contextlib
import hashlib
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time


def spawn_options():
    if os.name == "nt":
        return {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
    return {"start_new_session": True}


def stop_tree(process):
    """Stop owned descendants too, including those that outlive the group leader."""
    if os.name == "nt":
        subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10, check=False)
    else:
        try:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                pass
        except ProcessLookupError:
            pass
        finally:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
    process.wait(timeout=10)


@contextlib.contextmanager
def lease(resources, timeout=30):
    """OS locks release on death; never delete a lock file held by another run."""
    directory = Path(tempfile.gettempdir()) / f"qrow-test-locks-{os.getuid() if hasattr(os, 'getuid') else 'user'}"
    directory.mkdir(mode=0o700, exist_ok=True)
    handles = []
    deadline = time.monotonic() + timeout
    try:
        for resource in sorted(set(resources)):
            path = directory / hashlib.sha256(resource.encode()).hexdigest()
            handle = path.open("a+b")
            handles.append(handle)
            if path.stat().st_size == 0:
                handle.write(b"0")
                handle.flush()
            while True:
                try:
                    if os.name == "nt":
                        import msvcrt
                        handle.seek(0)
                        msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
                    else:
                        import fcntl
                        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise TimeoutError(f"Resource is in use: {resource}") from None
                    time.sleep(0.05)
        yield
    finally:
        for handle in reversed(handles):
            handle.close()
