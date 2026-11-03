"""Run Postgres connector and UI tests with a disposable Docker server."""
import argparse
import os
from pathlib import Path
import platform
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
IMAGE = "postgres:17-alpine@sha256:b0f9560a2de083e2cc7382e75f808c7381a32852a7ec49117deedb300e552b24"


def command(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--perf", action="store_true", help="Measure export transfer in an optimized build.")
    options = parser.parse_args()
    name = "qrow-e2e-postgres-" + uuid.uuid4().hex[:12]
    env = os.environ.copy()
    profile = "ci" if env.get("CI") else "default"
    # Compile before starting the server, so builds do not consume its resources.
    build = ("--profile", "perf") if options.perf else ()
    command("cargo", "test", "--locked", *build, "--no-default-features", "--test", "integration", "--no-run")
    if not options.perf and platform.system() == "Darwin":
        command("cargo", "test", "--locked", "--test", "e2e", "--no-run")
    target = Path(env.get("CARGO_TARGET_DIR", ROOT / "target"))
    fixture_root = target / "qtest" / "postgres"
    fixture_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="fixture-", dir=fixture_root) as temporary:
        path = Path(temporary)
        # Synthetic certificates identify only this local fixture.
        quiet = {"stdout": subprocess.DEVNULL, "stderr": subprocess.DEVNULL}
        command("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                "-subj", "/CN=Qrow Postgres test CA", "-keyout", str(path / "ca.key"),
                "-out", str(path / "ca.crt"), **quiet)
        command("openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=localhost",
                "-keyout", str(path / "server.key"), "-out", str(path / "server.csr"), **quiet)
        (path / "extensions").write_text("subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\n")
        command("openssl", "x509", "-req", "-in", str(path / "server.csr"), "-CA", str(path / "ca.crt"),
                "-CAkey", str(path / "ca.key"), "-CAcreateserial", "-days", "1", "-extfile", str(path / "extensions"),
                "-out", str(path / "server.crt"), **quiet)
        (path / "init.sh").write_text(
            "#!/bin/sh\ncp /fixture/server.* /var/lib/postgresql/\n"
            "chmod 600 /var/lib/postgresql/server.key\n"
            "cat >> \"$PGDATA/postgresql.conf\" <<'QROW_TLS'\n"
            "ssl=on\nssl_cert_file='/var/lib/postgresql/server.crt'\n"
            "ssl_key_file='/var/lib/postgresql/server.key'\nQROW_TLS\n")
        path.chmod(0o755)
        (path / "server.key").chmod(0o644)
        started = False
        try:
            command("docker", "run", "--detach", "--name", name,
                    "--publish", "127.0.0.1::5432", "--env", "POSTGRES_USER=qrow",
                    "--env", "POSTGRES_PASSWORD=qrow-test-password", "--env", "POSTGRES_DB=qrow",
                    "--env", "POSTGRES_INITDB_ARGS=--auth-host=scram-sha-256",
                    "--mount", f"type=bind,source={path},target=/fixture,readonly",
                    "--mount", f"type=bind,source={path / 'init.sh'},target=/docker-entrypoint-initdb.d/init.sh,readonly",
                    IMAGE)
            started = True
            port = command("docker", "port", name, "5432/tcp", capture_output=True).stdout.strip().rsplit(":", 1)[1]
            env.update(QROW_POSTGRES_FIXTURE=name, QROW_POSTGRES_PORT=port,
                       QROW_POSTGRES_CA=str(path / "ca.crt"))
            deadline = time.monotonic() + 60
            while True:
                ready = subprocess.run(["docker", "exec", "-e", "PGPASSWORD=qrow-test-password", name,
                                        "psql", "-h", "127.0.0.1", "-U", "qrow", "-d", "qrow", "-Atc", "SELECT 1"],
                                       capture_output=True, text=True, timeout=5)
                if ready.returncode == 0 and ready.stdout.strip() == "1":
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError("Postgres fixture did not answer SELECT 1 in time")
                time.sleep(0.2)
            if options.perf:
                command("cargo", "nextest", "run", "--locked", "--profile", profile, "--cargo-profile", "perf",
                        "--no-default-features", "--test", "integration", "--run-ignored", "ignored-only",
                        "--no-capture", "-E", "test(/^transfer_perf::postgres$/)", env=env)
                return
            command("cargo", "nextest", "run", "--locked", "--profile", profile, "--no-default-features", "--test", "integration",
                    "--run-ignored", "ignored-only", "-E", "test(/^postgres::/)", env=env)
            if platform.system() == "Darwin":
                command("cargo", "nextest", "run", "--locked", "--profile", profile, "--test", "e2e", "--run-ignored", "ignored-only",
                        "-E", "test(/^postgres::/)", env=env)
        finally:
            subprocess.run(["docker", "logs", name], check=False)
            if started:
                command("docker", "rm", "--force", "--volumes", name)


if __name__ == "__main__":
    main()
