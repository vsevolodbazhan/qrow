"""The server fixture must fail closed and scope every mutation to disposable servers."""
import http.server
import importlib.util
import json
from pathlib import Path
import shutil
import socketserver
import ssl
import subprocess
import sys
import tempfile
import threading
import types
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/e2e"))
spec = importlib.util.spec_from_file_location("fixture", ROOT / "scripts/e2e/fixture.py")
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class FakeServers:
    def __init__(self, answers):
        self.answers = answers
        self.users = []
        self.artifacts, self.tls_port, self.oidc_port = Path("/nonexistent"), 1, 2

    def check_alive(self):
        pass

    def beeline(self, user, _password):
        self.users.append(user)
        code, error = self.answers[user]
        return subprocess.CompletedProcess([], code, "", error)


class FixtureTests(unittest.TestCase):
    def test_both_runtimes_share_the_evidence_directory_with_tests(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence = str((Path(directory) / "executor-evidence").resolve())
            docker = fixture.DockerFixture("qrow-e2e-unit", 1, 2, directory)
            self.assertEqual(docker.env()["QROW_E2E_NATIVE_EVIDENCE"], evidence)
            with patch.object(fixture.subprocess, "run") as run:
                docker.compose("ps")
            self.assertEqual(run.call_args.kwargs["env"]["QROW_E2E_EVIDENCE"], evidence)
            native = fixture.NativeFixture("qrow-e2e-unit", 1, directory, [], [], {})
            self.assertEqual(native.env()["QROW_E2E_NATIVE_EVIDENCE"], str(Path(directory) / "executor-evidence"))

    def test_docker_fixture_rejects_an_unscoped_project_before_invoking_docker(self):
        for name in ["", "production", "qrow-e2e-", "qrow-e2e-a;ls", "../qrow-e2e-x"]:
            with self.subTest(name=name), patch.object(fixture.subprocess, "run") as run:
                with self.assertRaises(ValueError):
                    fixture.DockerFixture(name, 1, 1, "/tmp")
                run.assert_not_called()

    def test_mutations_are_scoped_to_the_fixture_project(self):
        docker = fixture.DockerFixture("qrow-e2e-unit", 23456, 1, "/tmp")
        with patch.object(fixture.subprocess, "run") as run:
            docker.kill_engine()
        args = run.call_args.args[0]
        self.assertEqual(args[:7], ["docker", "compose", "-f", str(fixture.COMPOSE), "-p", "qrow-e2e-unit", "exec"])
        self.assertIn("kyuubi", args)
        self.assertEqual(run.call_args.kwargs["env"]["QROW_E2E_PROJECT"], "qrow-e2e-unit")

    def test_local_processes_cannot_stop_engines(self):
        native = fixture.NativeFixture("qrow-e2e-unit", 1, "/tmp", [], [], {})
        with patch.object(fixture.subprocess, "run") as run:
            for action in (native.kill_engine, native.restart_server):
                with self.assertRaisesRegex(RuntimeError, "Docker runtime"):
                    action()
            run.assert_not_called()

    def test_authentication_failures_do_not_count_as_readiness(self):
        servers = FakeServers({"qrow": (1, "LDAP rejected")})
        with patch.object(fixture.time, "monotonic", side_effect=[0, 0, 0, 181]), patch.object(fixture.time, "sleep"), \
                patch.object(fixture, "secure_problem", return_value=None):
            with self.assertRaisesRegex(RuntimeError, "LDAP rejected"):
                fixture.wait_ready(servers)

    def test_readiness_starts_only_the_engine_of_the_test_user(self):
        servers = FakeServers({"qrow": (0, "")})
        with patch.object(fixture, "secure_problem", return_value=None):
            fixture.wait_ready(servers)
        self.assertEqual(servers.users, [fixture.TEST_USER])

    def test_readiness_needs_the_oidc_provider_and_the_tls_proxy_before_sql(self):
        servers = FakeServers({"qrow": (0, "")})
        with patch.object(fixture.time, "monotonic", side_effect=[0, 0, 181]), patch.object(fixture.time, "sleep"), \
                patch.object(fixture, "secure_problem", return_value="ConnectionRefusedError"):
            with self.assertRaisesRegex(RuntimeError, "ConnectionRefusedError"):
                fixture.wait_ready(servers)
        self.assertEqual(servers.users, [])

    def test_both_runtimes_export_the_tls_and_oidc_endpoints(self):
        with tempfile.TemporaryDirectory() as directory:
            ca = str((Path(directory) / "security/ca.pem").resolve())
            docker = fixture.DockerFixture("qrow-e2e-unit", 1, 2, directory, tls_port=3, oidc_port=4)
            native = fixture.NativeFixture("qrow-e2e-unit", 2, directory, [], [], {}, tls_port=3, oidc_port=4)
            for runtime in (docker, native):
                with self.subTest(runtime=runtime.runtime):
                    env = runtime.env()
                    self.assertEqual(env["QROW_E2E_TLS_PORT"], "3")
                    self.assertEqual(env["QROW_E2E_OIDC_ISSUER"], "https://127.0.0.1:4")
                    self.assertEqual(env["QROW_E2E_TLS_CA"], ca)
                    self.assertTrue(Path(env["QROW_E2E_TLS_CA"]).is_absolute())
            with patch.object(fixture.subprocess, "run") as run:
                docker.compose("ps")
            env = run.call_args.kwargs["env"]
            self.assertEqual((env["QROW_E2E_TLS_BIND_PORT"], env["QROW_E2E_OIDC_PORT"]), ("3", "4"))
            self.assertEqual(env["QROW_E2E_SECURITY"], str((Path(directory) / "security").resolve()))

    def test_state_of_an_older_fixture_loads_without_tls_ports(self):
        docker = fixture.DockerFixture.from_state({"project": "qrow-e2e-unit", "bind_port": 1, "port": 2,
                                                   "artifacts": "/tmp"})
        native = fixture.NativeFixture.from_state({"project": "qrow-e2e-unit", "port": 2, "artifacts": "/tmp",
                                                   "processes": [], "beeline": [], "server_env": {}})
        # Port 0 never answers, so such a fixture is not healthy and is not reused.
        self.assertEqual((docker.tls_port, docker.oidc_port, native.tls_port, native.oidc_port), (0, 0, 0, 0))

    def test_kyuubi_server_jars_include_the_fixture_jar_without_changing_the_download(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "kyuubi/jars").mkdir(parents=True)
            (root / "kyuubi/jars/kyuubi-server.jar").write_text("server")
            (root / "artifacts").mkdir()
            jar = root / "artifacts/qrow-fixture.jar"
            jar.write_text("fixture")
            for _ in range(2):
                jars = fixture.kyuubi_jar_dir(root / "artifacts", root / "kyuubi", jar)
            self.assertEqual(sorted(path.name for path in jars.iterdir()), ["kyuubi-server.jar", "qrow-fixture.jar"])
            self.assertEqual((jars / "qrow-fixture.jar").read_text(), "fixture")
            self.assertEqual(sorted(path.name for path in (root / "kyuubi/jars").iterdir()), ["kyuubi-server.jar"])

    def test_kyuubi_uses_the_token_or_ldap_authenticator(self):
        config = (fixture.SERVER / "kyuubi-defaults.conf").read_text().splitlines()
        self.assertIn("kyuubi.authentication CUSTOM", config)
        self.assertIn("kyuubi.authentication.custom.class io.qrow.fixture.TokenOrLdap", config)
        self.assertTrue(any(line.startswith("kyuubi.authentication.ldap.url ") for line in config))
        self.assertIn("TokenOrLdap.java", fixture.NativeFixture.SOURCES)

    def test_state_round_trip_keeps_the_scope(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.json"
            fixture.save(fixture.DockerFixture("qrow-e2e-unit", 2, 3, directory), path)
            loaded = fixture.load(path)
            evidence = str((Path(directory) / "executor-evidence").resolve())
            ca = str((Path(directory) / "security/ca.pem").resolve())
        self.assertEqual((loaded.project, loaded.bind_port, loaded.port), ("qrow-e2e-unit", 2, 3))
        self.assertEqual(loaded.env(), {"QROW_E2E_PROJECT": "qrow-e2e-unit", "QROW_E2E_PORT": "3",
                                        "QROW_E2E_NATIVE_EVIDENCE": evidence, "QROW_E2E_TLS_PORT": "0",
                                        "QROW_E2E_OIDC_ISSUER": "https://127.0.0.1:0",
                                        "QROW_E2E_TLS_CA": ca})

    def test_state_round_trip_keeps_the_tls_and_oidc_ports(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.json"
            for runtime in (fixture.DockerFixture("qrow-e2e-unit", 2, 3, directory, tls_port=5, oidc_port=6),
                            fixture.NativeFixture("qrow-e2e-unit", 3, directory, [], [], {}, tls_port=5,
                                                  oidc_port=6)):
                with self.subTest(runtime=runtime.runtime):
                    fixture.save(runtime, path)
                    loaded = fixture.load(path)
                    self.assertEqual((loaded.tls_port, loaded.oidc_port), (5, 6))
                    self.assertEqual(loaded.env(), runtime.env())

OPENSSL_CA = """[req]
distinguished_name = name
x509_extensions = ca
prompt = no
[name]
CN = Qrow unit CA
[ca]
basicConstraints = critical,CA:TRUE
keyUsage = critical,keyCertSign,cRLSign
subjectKeyIdentifier = hash
"""
OPENSSL_SERVER = """basicConstraints = CA:FALSE
keyUsage = critical,digitalSignature,keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = IP:127.0.0.1
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
"""


def certificate_authority(directory, name):
    """A CA and a server certificate for 127.0.0.1, like the fixture makes with keytool."""
    root = Path(directory) / name
    root.mkdir()
    (root / "ca.cnf").write_text(OPENSSL_CA)
    (root / "server.cnf").write_text(OPENSSL_SERVER)

    def openssl(*args):
        subprocess.run(["openssl", *args], cwd=root, check=True, capture_output=True)

    openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-config", "ca.cnf",
            "-keyout", "ca.key", "-out", "ca.pem")
    openssl("req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=localhost",
            "-keyout", "server.key", "-out", "server.csr")
    openssl("x509", "-req", "-in", "server.csr", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial",
            "-days", "2", "-extfile", "server.cnf", "-out", "server.pem")
    return root


class LoopbackServer(http.server.ThreadingHTTPServer):
    def server_bind(self):
        # HTTPServer.server_bind looks up the host name, which can wait for DNS.
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]


@unittest.skipUnless(shutil.which("openssl"), "needs the openssl command")
class SecureReadinessTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.certificates = tempfile.TemporaryDirectory()
        cls.authority = certificate_authority(cls.certificates.name, "fixture")
        cls.other = certificate_authority(cls.certificates.name, "other") / "ca.pem"

    @classmethod
    def tearDownClass(cls):
        cls.certificates.cleanup()

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        authority = self.authority
        self.artifacts = Path(self.directory.name) / "artifacts"
        fixture.security_dir(self.artifacts).mkdir(parents=True)
        shutil.copyfile(authority / "ca.pem", fixture.ca_path(self.artifacts))
        reported = {}

        class Discovery(http.server.BaseHTTPRequestHandler):
            def do_GET(self):  # noqa: N802 - http.server API
                body = json.dumps({"issuer": reported["issuer"]}).encode()
                self.send_response(200 if self.path == "/.well-known/openid-configuration" else 404)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_):
                pass

        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(authority / "server.pem", authority / "server.key")
        self.server = LoopbackServer(("127.0.0.1", 0), Discovery)
        # Handshake-only and rejected connections are expected here.
        self.server.handle_error = lambda *_: None
        self.server.socket = context.wrap_socket(self.server.socket, server_side=True)
        self.port = self.server.server_address[1]
        reported["issuer"] = fixture.issuer(self.port)
        self.reported = reported
        thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.servers = types.SimpleNamespace(artifacts=self.artifacts, oidc_port=self.port, tls_port=self.port)

    def test_the_fixture_ca_verifies_the_provider_and_the_proxy(self):
        self.assertIsNone(fixture.secure_problem(self.servers))
        self.assertIn(fixture.tls_handshake(self.port, fixture.ca_path(self.artifacts)), ("TLSv1.2", "TLSv1.3"))

    def test_another_ca_is_not_trusted(self):
        with self.assertRaises(ssl.SSLCertVerificationError):
            fixture.tls_handshake(self.port, self.other)
        shutil.copyfile(self.other, fixture.ca_path(self.artifacts))
        self.assertIn("certificate verify failed", fixture.secure_problem(self.servers))

    def test_the_discovery_document_must_name_the_issuer(self):
        self.reported["issuer"] = "https://127.0.0.1:1"
        self.assertIn("reports issuer", fixture.secure_problem(self.servers))

    def test_a_missing_ca_or_a_closed_port_is_not_ready(self):
        closed = types.SimpleNamespace(artifacts=self.artifacts, oidc_port=self.port, tls_port=fixture.free_ports(1)[0])
        self.assertIn("ConnectionRefusedError", fixture.secure_problem(closed))
        fixture.ca_path(self.artifacts).unlink()
        self.assertIn("No CA certificate", fixture.secure_problem(self.servers))


class DriverTests(unittest.TestCase):
    def test_native_e2e_package_mode_validates_bundle(self):
        driver = (ROOT / "scripts/e2e/driver.sh").read_text()
        self.assertIn("QROW_E2E_REUSE_MACOS_PACKAGE", driver)
        self.assertIn("ditto -x -k", driver)
        self.assertIn("QROW_E2E_BUNDLE/Contents/MacOS/qrow", driver)

    def test_shell_scripts_build_in_the_cargo_target_directory(self):
        # Hook snapshots set CARGO_TARGET_DIR, so a fixed target/ path reads another build.
        for script in ("scripts/e2e/driver.sh", "scripts/package/macos.sh"):
            with self.subTest(script=script):
                text = (ROOT / script).read_text()
                self.assertEqual(text.count('qrow_target_dir="${CARGO_TARGET_DIR:-target}"'), 1)
                paths = [line for line in text.splitlines() if "target/" in line]
                self.assertEqual(paths, [])

    def test_only_preflight_mode_checks_permissions_and_only_the_driver_run_cleans_up(self):
        driver = (ROOT / "scripts/e2e/driver.sh").read_text()
        preflight = driver.index('if [ "${1:-}" = --preflight ]; then')
        self.assertEqual(driver.count('"$qrow_driver" --preflight'), 1)
        self.assertLess(preflight, driver.index('"$qrow_driver" --preflight'))
        self.assertLess(driver.index('"$qrow_driver" --preflight'), driver.index("    exit 0\nfi\n", preflight))
        prepare_exit = driver.index('test "${1:-}" != --prepare || exit 0')
        self.assertLess(prepare_exit, driver.index("trap cleanup 0"))
        self.assertEqual(driver.count("trap cleanup 0"), 1)
