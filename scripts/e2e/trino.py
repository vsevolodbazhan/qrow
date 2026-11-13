"""Run Trino connector and real-window tests with a disposable Docker coordinator."""
import json
import base64
import argparse
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
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/qtest"))
import builds  # noqa: E402

IMAGE = "trinodb/trino:483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534"

STORAGE_IMAGE = "adobe/s3mock:5.2.3@sha256:ab01a6946750f451ca215a47e91030695b260e4003b8a5a6201d25029b8fca92"


# The real-window tests run on macOS and Linux. On Linux, GPUI's test support
# is behind the ui-test feature.
UI_TEST_FEATURE = ("--features", "ui-test") if platform.system() == "Linux" else ()


def command(*args, **kwargs):
    if args[:3] == ("cargo", "nextest", "run"):
        # A layer selects scenarios, not different fixture behavior.
        layer = os.environ.get("QROW_CONNECTOR_LAYER", "all")
        core = "--no-default-features" in args
        if layer != "all" and core != (layer == "core"):
            return None
        return builds.run(list(args), kwargs.get("env"))
    return subprocess.run(args, check=True, text=True, **kwargs)


def accepts_queries(origin, context):
    """True when the coordinator finished startup and a query that reads task results succeeds.

    An accepted statement does not prove readiness. The task result endpoint can
    still answer 503 while the coordinator initializes."""
    headers = {"X-Trino-User": "qrow", "Authorization": "Basic cXJvdzpxcm93LXRlc3QtcGFzc3dvcmQ="}
    try:
        with urllib.request.urlopen(origin + "/v1/info", context=context, timeout=5) as response:
            if json.load(response).get("starting", True) is not False:
                return False
        request = urllib.request.Request(origin + "/v1/statement", data=b"SELECT count(*) FROM tpch.tiny.region",
                                         headers=headers)
        deadline = time.monotonic() + 30
        rows = []
        while request is not None:
            with urllib.request.urlopen(request, context=context, timeout=5) as response:
                body = json.load(response)
            if body.get("error"):
                return False
            # Rows can arrive on any page. The last page often has none.
            rows.extend(body.get("data", []))
            if "nextUri" not in body:
                return rows == [[5]]
            if time.monotonic() >= deadline:
                return False
            request = urllib.request.Request(body["nextUri"], headers=headers)
    except (urllib.error.URLError, OSError, ValueError):
        return False
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--perf", action="store_true", help="Measure export transfer in an optimized build.")
    parser.add_argument("--layer", choices=("all", "core", "ui"), default="all")
    options = parser.parse_args()
    if options.perf and options.layer == "ui":
        parser.error("--perf requires core tests")
    os.environ["QROW_CONNECTOR_LAYER"] = options.layer
    name = "qrow-e2e-trino-" + uuid.uuid4().hex[:12]
    storage_name = name + "-storage"
    env = os.environ.copy()
    profile = "ci" if env.get("CI") else "default"
    build = ("--profile", "perf") if options.perf else ()
    if not os.environ.get("QROW_NEXTEST_RUNTIME"):
        if options.layer != "ui":
            command("cargo", "test", "--locked", *build, "--no-default-features", "--test", "integration", "--no-run")
        if not options.perf and options.layer != "core":
            command("cargo", "test", "--locked", *UI_TEST_FEATURE, "--test", "e2e", "--no-run")
    target = Path(env.get("CARGO_TARGET_DIR", ROOT / "target"))
    fixture_root = target / "qtest" / "trino"
    fixture_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="fixture-", dir=fixture_root) as temporary:
        path = Path(temporary)
        (path / "catalog").mkdir()
        quiet = {"stdout": subprocess.DEVNULL, "stderr": subprocess.DEVNULL}
        # An explicit configuration gives OpenSSL and LibreSSL the same CA. Python
        # 3.13 verifies X.509 strictly and needs both key identifiers.
        (path / "ca.cnf").write_text("[req]\ndistinguished_name=dn\nx509_extensions=ca\nprompt=no\n[dn]\nCN=Qrow Trino test CA\n"
                                     "[ca]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n"
                                     "subjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid:always\n")
        command("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                "-config", str(path / "ca.cnf"), "-keyout", str(path / "ca.key"),
                "-out", str(path / "ca.crt"), **quiet)
        command("openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=localhost",
                "-keyout", str(path / "server.key"), "-out", str(path / "server.csr"), **quiet)
        (path / "extensions").write_text("subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=critical,CA:FALSE\n"
                                           "subjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid,issuer\n")
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
            "discovery.uri=http://localhost:8080\n"
            "query.client.timeout=5s\n"
            "http-server.log.path=/tmp/qrow-http-request.log\n"
            "http-server.log.immediate-flush=true\n"
            "http-server.log.compression.enabled=false\n")
        (path / "node.properties").write_text("node.environment=test\nnode.id=qrow-test\nnode.data-dir=/tmp/trino-data\n")
        (path / "jvm.config").write_text("-server\n-Xmx2G\n-XX:+UseG1GC\n-XX:+ExitOnOutOfMemoryError\n-Djdk.attach.allowAttachSelf=true\n-Djavax.net.ssl.trustStore=/etc/trino/fixture-trust.p12\n-Djavax.net.ssl.trustStorePassword=synthetic-fixture-trust\n")
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
        storage_started = False
        try:
            # Explicit bindings stay stable when Docker restarts the container.
            with socket.socket() as coordinator_socket, socket.socket() as provider_socket, socket.socket() as storage_socket:
                coordinator_socket.bind(("127.0.0.1", 0))
                provider_socket.bind(("127.0.0.1", 0))
                storage_socket.bind(("127.0.0.1", 0))
                coordinator_port = coordinator_socket.getsockname()[1]
                provider_port = provider_socket.getsockname()[1]
                storage_port = storage_socket.getsockname()[1]
            command("openssl", "pkcs12", "-export", "-name", "fixture", "-in", str(path / "server.crt"),
                    "-inkey", str(path / "server.key"), "-certfile", str(path / "ca.crt"), "-out", str(path / "storage.p12"),
                    "-passout", "pass:synthetic-fixture-keystore", **quiet)
            (path / "storage.p12").chmod(0o644)
            command("docker", "run", "--rm", "--user", "0", "--entrypoint", "keytool",
                    "--mount", f"type=bind,source={path},target=/etc/trino", IMAGE,
                    "-importcert", "-noprompt", "-alias", "fixture", "-file", "/etc/trino/ca.crt",
                    "-keystore", "/etc/trino/fixture-trust.p12", "-storetype", "PKCS12", "-storepass", "synthetic-fixture-trust")
            (path / "spooling-manager.properties").write_text(
                "spooling-manager.name=filesystem\nfs.s3.enabled=true\nfs.location=s3://qrow-spooling/segments/\n"
                f"s3.endpoint=https://localhost:{storage_port}\n"
                "s3.region=us-east-1\ns3.aws-access-key=synthetic-storage-user\ns3.aws-secret-key=synthetic-storage-secret\n"
                "s3.path-style-access=true\nfs.segment.encryption=false\n")
            with (path / "config.properties").open("a") as output:
                output.write("protocol.spooling.enabled=true\nprotocol.spooling.shared-secret-key=" + base64.b64encode(os.urandom(32)).decode() + "\n")
            command("docker", "run", "--detach", "--name", storage_name, "--memory", "1g", "--cpus", "1",
                    "--publish", f"127.0.0.1:{coordinator_port}:8443", "--publish", f"127.0.0.1:{provider_port}:9443",
                    "--publish", f"127.0.0.1:{storage_port}:{storage_port}",
                    "--mount", f"type=bind,source={path},target=/fixtures,readonly",
                    "--env", "JAVA_TOOL_OPTIONS=-Xmx512m", "--env", f"SERVER_PORT={storage_port}",
                    "--env", "SERVER_SSL_KEY_STORE=file:/fixtures/storage.p12", "--env", "SERVER_SSL_KEY_STORE_TYPE=PKCS12",
                    "--env", "SERVER_SSL_KEY_STORE_PASSWORD=synthetic-fixture-keystore", "--env", "SERVER_SSL_KEY_PASSWORD=synthetic-fixture-keystore",
                    "--env", "SERVER_SSL_KEY_ALIAS=fixture", "--env", "COM_ADOBE_TESTING_S3MOCK_STORE_INITIAL_BUCKETS=qrow-spooling",
                    "--env", "COM_ADOBE_TESTING_S3MOCK_MAX_PAYLOAD_SIZE=64MB",
                    "--env", "SERVER_TOMCAT_ACCESSLOG_ENABLED=true", "--env", "SERVER_TOMCAT_ACCESSLOG_BUFFERED=false",
                    "--env", "SERVER_TOMCAT_ACCESSLOG_DIRECTORY=/tmp/qrow-storage-requests",
                    "--env", "SERVER_TOMCAT_ACCESSLOG_PATTERN=%m %U %s", STORAGE_IMAGE)
            storage_started = True
            context = ssl.create_default_context(cafile=str(path / "ca.crt"))
            deadline = time.monotonic() + 120
            while True:
                try:
                    with urllib.request.urlopen(f"https://localhost:{storage_port}/qrow-spooling", context=context, timeout=5) as response:
                        if response.status == 200:
                            break
                except (urllib.error.URLError, OSError):
                    pass
                if time.monotonic() >= deadline:
                    raise RuntimeError("Trino storage fixture did not create its bucket in time")
                time.sleep(0.2)
            # Parallel tests hold several large results and spooled segments at
            # once. A 768 MB heap ran out of memory locally and needed major
            # collections in CI. The JVM also needs memory outside the heap.
            command("docker", "run", "--detach", "--name", name, "--memory", "3g", "--cpus", "2",
                    "--network", f"container:{storage_name}", "--mount", f"type=bind,source={path},target=/etc/trino,readonly", IMAGE)
            started = True
            port = str(coordinator_port)
            env.update(QROW_TRINO_FIXTURE=name, QROW_TRINO_PORT=port, QROW_TRINO_CA=str(path / "ca.crt"))
            env.update(QROW_TRINO_STORAGE=storage_name, QROW_TRINO_STORAGE_PORT=str(storage_port))
            deadline = time.monotonic() + 120
            while not accepts_queries(f"https://localhost:{port}", context):
                if time.monotonic() >= deadline:
                    raise RuntimeError("Trino fixture did not complete a tpch query in time")
                time.sleep(0.2)
            if options.perf:
                command("cargo", "nextest", "run", "--locked", "--profile", profile, "--cargo-profile", "perf",
                        "--no-default-features", "--test", "integration", "--run-ignored", "ignored-only",
                        "--no-capture", "-E", "test(/^transfer_perf::trino$/)", env=env)
                return
            command("cargo", "nextest", "run", "--locked", "--profile", profile, "--no-default-features", "--test", "integration",
                    "--run-ignored", "ignored-only", "-E", "test(/^trino::/) & not test(/external_browser/)", env=env)
            command("cargo", "nextest", "run", "--locked", "--profile", profile, *UI_TEST_FEATURE, "--test", "e2e",
                    "--run-ignored", "ignored-only", "-E", "test(/^trino::/) & not test(/external_browser/)", env=env)
            # The same disposable coordinator now delegates login to its
            # synthetic confidential OIDC client. Only this fixture changes.
            oidc_port = command("docker", "port", storage_name, "9443/tcp", capture_output=True).stdout.strip().rsplit(":", 1)[1]
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
            command("docker", "restart", name)
            command("docker", "exec", "--detach", "--env", f"QROW_FIXTURE_TRINO_ORIGIN={origin}", name,
                    "/bin/sh", "-c", "java -Xmx128m /etc/trino/io/qrow/fixture/Oidc.java 9443 /etc/trino/oidc.p12 synthetic-fixture-keystore /tmp/trino-jwks.json 0.0.0.0 > /tmp/trino-oidc.log 2>&1")
            deadline = time.monotonic() + 120
            while True:
                try:
                    with urllib.request.urlopen(f"https://localhost:{oidc_port}/jwks", context=context, timeout=5):
                        pass
                    # Authentication challenges can precede OAuth2 client
                    # initialization during catalog startup.
                    with urllib.request.urlopen(origin + "/v1/info", context=context, timeout=5) as response:
                        ready = json.load(response).get("starting", True) is False
                    if ready:
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
            command("cargo", "nextest", "run", "--locked", "--profile", profile, *UI_TEST_FEATURE, "--test", "e2e",
                    "--run-ignored", "ignored-only", "-E", "test(/^trino::/) & test(/external_browser/)", env=env)
        finally:
            if started:
                subprocess.run(["docker", "exec", name, "/bin/sh", "-c", "test ! -f /tmp/trino-oidc.log || cat /tmp/trino-oidc.log"], check=False)
            subprocess.run(["docker", "logs", name], check=False)
            try:
                if started:
                    command("docker", "rm", "--force", "--volumes", name)
            finally:
                if storage_started:
                    subprocess.run(["docker", "logs", storage_name], check=False)
                    command("docker", "rm", "--force", "--volumes", storage_name)


if __name__ == "__main__":
    main()
