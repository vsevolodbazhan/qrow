"""Run Trino connector and real-window tests with a disposable Docker coordinator."""
import os
from pathlib import Path
import platform
import socket
import ssl
import shutil
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[2]
IMAGE = "trinodb/trino:483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534"


def command(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def main():
    name = "qrow-e2e-trino-" + uuid.uuid4().hex[:12]
    env = os.environ.copy()
    profile = "ci" if env.get("CI") else "default"
    command("cargo", "test", "--locked", "--no-default-features", "--test", "integration", "--no-run")
    if platform.system() == "Darwin":
        command("cargo", "test", "--locked", "--test", "e2e", "--no-run")
    target = Path(env.get("CARGO_TARGET_DIR", ROOT / "target"))
    fixture_root = target / "qtest" / "trino"
    fixture_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="fixture-", dir=fixture_root) as temporary:
        path = Path(temporary)
        (path / "catalog").mkdir()
        quiet = {"stdout": subprocess.DEVNULL, "stderr": subprocess.DEVNULL}
        command("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                "-subj", "/CN=Qrow Trino test CA", "-keyout", str(path / "ca.key"),
                "-out", str(path / "ca.crt"), "-addext", "keyUsage=critical,keyCertSign,cRLSign", **quiet)
        command("openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=localhost",
                "-keyout", str(path / "server.key"), "-out", str(path / "server.csr"), **quiet)
        (path / "extensions").write_text("subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\n")
        command("openssl", "x509", "-req", "-in", str(path / "server.csr"), "-CA", str(path / "ca.crt"),
                "-CAkey", str(path / "ca.key"), "-CAcreateserial", "-days", "1", "-extfile", str(path / "extensions"),
                "-out", str(path / "server.crt"), **quiet)
        (path / "server.pem").write_bytes((path / "server.key").read_bytes() + (path / "server.crt").read_bytes())
        (path / "config.properties").write_text(
            "coordinator=true\nnode-scheduler.include-coordinator=true\n"
            "http-server.http.port=8080\nhttp-server.https.enabled=true\nhttp-server.https.port=8443\n"
            "http-server.https.keystore.path=/etc/trino/server.pem\n"
            "http-server.authentication.type=PASSWORD\n"
            "internal-communication.shared-secret=qrow-trino-synthetic-internal-secret\n"
            "discovery.uri=http://localhost:8080\n")
        (path / "node.properties").write_text("node.environment=test\nnode.id=qrow-test\nnode.data-dir=/tmp/trino-data\n")
        (path / "jvm.config").write_text("-server\n-Xmx768M\n-XX:+UseG1GC\n-XX:+ExitOnOutOfMemoryError\n-Djdk.attach.allowAttachSelf=true\n")
        (path / "log.properties").write_text("io.trino=INFO\n")
        (path / "password-authenticator.properties").write_text("password-authenticator.name=file\nfile.password-file=/etc/trino/password.db\n")
        (path / "password.db").write_text("qrow:$2y$10$XGTex8TgykMhNp//95n31Okw76IAjMLFdLSLwnxZh4STmyR8RwHpK\n")
        (path / "catalog" / "tpch.properties").write_text("connector.name=tpch\n")
        (path / "catalog" / "memory.properties").write_text("connector.name=memory\n")
        path.chmod(0o755)
        oidc_source = path / "io" / "qrow" / "fixture" / "Oidc.java"
        oidc_source.parent.mkdir(parents=True)
        shutil.copyfile(ROOT / "tests/fixture/server/Oidc.java", oidc_source)
        command("openssl", "pkcs12", "-export", "-in", str(path / "server.crt"), "-inkey", str(path / "server.key"),
                "-out", str(path / "oidc.p12"), "-passout", "pass:synthetic-fixture-keystore", **quiet)
        (path / "oidc.p12").chmod(0o644)
        started = False
        try:
            # Explicit bindings stay stable when Docker restarts the container.
            with socket.socket() as coordinator_socket, socket.socket() as provider_socket:
                coordinator_socket.bind(("127.0.0.1", 0))
                provider_socket.bind(("127.0.0.1", 0))
                coordinator_port = coordinator_socket.getsockname()[1]
                provider_port = provider_socket.getsockname()[1]
            command("docker", "run", "--detach", "--name", name, "--memory", "2g", "--cpus", "2",
                    "--publish", f"127.0.0.1:{coordinator_port}:8443", "--publish", f"127.0.0.1:{provider_port}:9443", "--mount", f"type=bind,source={path},target=/etc/trino,readonly", IMAGE)
            started = True
            port = command("docker", "port", name, "8443/tcp", capture_output=True).stdout.strip().rsplit(":", 1)[1]
            env.update(QROW_TRINO_FIXTURE=name, QROW_TRINO_PORT=port, QROW_TRINO_CA=str(path / "ca.crt"))
            context = ssl.create_default_context(cafile=str(path / "ca.crt"))
            deadline = time.monotonic() + 120
            while True:
                request = urllib.request.Request(f"https://localhost:{port}/v1/statement", data=b"SELECT 1",
                                                 headers={"X-Trino-User": "qrow", "Authorization": "Basic cXJvdzpxcm93LXRlc3QtcGFzc3dvcmQ="})
                try:
                    with urllib.request.urlopen(request, context=context, timeout=5) as response:
                        if response.status == 200:
                            break
                except (urllib.error.URLError, OSError):
                    pass
                if time.monotonic() >= deadline:
                    raise RuntimeError("Trino fixture did not accept SELECT 1 in time")
                time.sleep(0.2)
            command("cargo", "nextest", "run", "--locked", "--profile", profile, "--no-default-features", "--test", "integration",
                    "--run-ignored", "ignored-only", "-E", "test(/^trino::/) & not test(/external_browser/)", env=env)
            if platform.system() == "Darwin":
                command("cargo", "nextest", "run", "--locked", "--profile", profile, "--test", "e2e",
                        "--run-ignored", "ignored-only", "-E", "test(/^trino::/) & not test(/external_browser/)", env=env)
            # The same disposable coordinator now delegates login to its
            # synthetic confidential OIDC client. Only this fixture changes.
            oidc_port = command("docker", "port", name, "9443/tcp", capture_output=True).stdout.strip().rsplit(":", 1)[1]
            origin = f"https://localhost:{port}"
            env.update(QROW_TRINO_OAUTH="1", QROW_TRINO_OIDC_PORT=oidc_port)
            config = (path / "config.properties").read_text().replace("http-server.authentication.type=PASSWORD", "http-server.authentication.type=OAUTH2")
            config += (
                "http-server.authentication.oauth2.oidc.discovery=false\n"
                "http-server.authentication.oauth2.issuer=https://127.0.0.1:9443\n"
                "http-server.authentication.oauth2.client-id=trino\n"
                "http-server.authentication.oauth2.client-secret=synthetic-trino-client-secret\n"
                f"http-server.authentication.oauth2.auth-url=https://localhost:{oidc_port}/authorize\n"
                "http-server.authentication.oauth2.token-url=https://localhost:9443/token\n"
                "http-server.authentication.oauth2.jwks-url=https://localhost:9443/jwks\n"
                "http-server.authentication.oauth2.principal-field=preferred_username\n"
                "http-server.authentication.oauth2.refresh-tokens=true\n")
            (path / "config.properties").write_text(config)
            command("docker", "exec", name, "keytool", "-importcert", "-noprompt", "-alias", "fixture", "-file", "/etc/trino/ca.crt",
                    "-keystore", "/tmp/trino-trust.p12", "-storetype", "PKCS12", "-storepass", "synthetic-fixture-trust")
            with (path / "jvm.config").open("a") as output:
                output.write("-Djavax.net.ssl.trustStore=/tmp/trino-trust.p12\n-Djavax.net.ssl.trustStorePassword=synthetic-fixture-trust\n")
            command("docker", "restart", name)
            command("docker", "exec", "--detach", "--env", f"QROW_FIXTURE_TRINO_ORIGIN={origin}", name,
                    "/bin/sh", "-c", "java -Xmx128m /etc/trino/io/qrow/fixture/Oidc.java 9443 /etc/trino/oidc.p12 synthetic-fixture-keystore /tmp/trino-jwks.json 0.0.0.0 > /tmp/trino-oidc.log 2>&1")
            deadline = time.monotonic() + 120
            while True:
                try:
                    with urllib.request.urlopen(f"https://localhost:{oidc_port}/jwks", context=context, timeout=5):
                        pass
                    urllib.request.urlopen(urllib.request.Request(origin + "/v1/statement", data=b"SELECT 1", headers={"X-Trino-User": "alice"}), context=context, timeout=5)
                except urllib.error.HTTPError as error:
                    if error.code == 401 and "x_token_server" in error.headers.get("WWW-Authenticate", ""):
                        break
                except (urllib.error.URLError, OSError):
                    pass
                if time.monotonic() >= deadline:
                    raise RuntimeError("Trino OAuth2 fixture did not issue an external authentication challenge")
                time.sleep(0.2)
            command("cargo", "nextest", "run", "--locked", "--profile", profile, "--no-default-features", "--test", "integration",
                    "--run-ignored", "ignored-only", "-E", "test(/^trino::/) & test(/external_browser/)", env=env)
            if platform.system() == "Darwin":
                command("cargo", "nextest", "run", "--locked", "--profile", profile, "--test", "e2e",
                        "--run-ignored", "ignored-only", "-E", "test(/^trino::/) & test(/external_browser/)", env=env)
        finally:
            if started:
                subprocess.run(["docker", "exec", name, "/bin/sh", "-c", "test ! -f /tmp/trino-oidc.log || cat /tmp/trino-oidc.log"], check=False)
            subprocess.run(["docker", "logs", name], check=False)
            if started:
                command("docker", "rm", "--force", "--volumes", name)


if __name__ == "__main__":
    main()
