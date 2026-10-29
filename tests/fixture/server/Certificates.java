package io.qrow.fixture;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/**
 * Synthetic certificate authority and server certificate for the fixture TLS endpoints.
 *
 * <p>Usage: {@code Certificates <directory>}. Writes {@code ca.pem} (the only root that tests
 * trust), {@code server.p12} (server key with the chain server -> CA, for the Java servers), and
 * {@code truststore.p12} (the CA only, for Java clients such as beeline). Every run makes new keys.
 * Never shipped with Qrow.
 */
public final class Certificates {
    /** Synthetic password of the fixture keystores. It protects nothing. */
    public static final String PASSWORD = "qrow-fixture-tls";
    private static final String SAN = "SAN=ip:127.0.0.1,dns:localhost,dns:kyuubi,dns:kyuubi-tls,dns:oidc";

    private Certificates() {}

    public static void main(String[] args) throws Exception {
        Path directory = Path.of(args[0]).toAbsolutePath();
        Files.createDirectories(directory);
        Path work = Files.createTempDirectory(directory, ".certificates-");
        Path ca = work.resolve("ca.p12");
        Path server = work.resolve("server.p12");
        keytool("-genkeypair", "-alias", "ca", "-keyalg", "RSA", "-keysize", "2048",
            "-dname", "CN=Qrow Fixture CA,O=Qrow tests", "-startdate", "-1d", "-validity", "30",
            "-ext", "bc:c", "-ext", "KU:c=keyCertSign,cRLSign",
            "-keystore", ca.toString(), "-storetype", "PKCS12", "-storepass", PASSWORD);
        keytool("-exportcert", "-rfc", "-alias", "ca", "-keystore", ca.toString(), "-storepass", PASSWORD,
            "-file", work.resolve("ca.pem").toString());
        keytool("-genkeypair", "-alias", "server", "-keyalg", "RSA", "-keysize", "2048",
            "-dname", "CN=localhost,O=Qrow tests", "-validity", "30",
            "-keystore", server.toString(), "-storetype", "PKCS12", "-storepass", PASSWORD);
        keytool("-certreq", "-alias", "server", "-keystore", server.toString(), "-storepass", PASSWORD,
            "-file", work.resolve("server.csr").toString());
        keytool("-gencert", "-rfc", "-alias", "ca", "-keystore", ca.toString(), "-storepass", PASSWORD,
            "-infile", work.resolve("server.csr").toString(), "-outfile", work.resolve("server.pem").toString(),
            "-startdate", "-1d", "-validity", "30", "-ext", SAN, "-ext", "EKU=serverAuth",
            "-ext", "KU:c=digitalSignature,keyEncipherment");
        // The CA entry lets keytool build the chain server -> CA from the signed reply.
        keytool("-importcert", "-noprompt", "-alias", "ca", "-file", work.resolve("ca.pem").toString(),
            "-keystore", server.toString(), "-storepass", PASSWORD);
        keytool("-importcert", "-alias", "server", "-file", work.resolve("server.pem").toString(),
            "-keystore", server.toString(), "-storepass", PASSWORD);
        keytool("-importcert", "-noprompt", "-alias", "ca", "-file", work.resolve("ca.pem").toString(),
            "-keystore", work.resolve("truststore.p12").toString(), "-storetype", "PKCS12",
            "-storepass", PASSWORD);
        for (String name : List.of("server.p12", "truststore.p12", "ca.pem")) {
            Files.move(work.resolve(name), directory.resolve(name),
                java.nio.file.StandardCopyOption.REPLACE_EXISTING, java.nio.file.StandardCopyOption.ATOMIC_MOVE);
        }
        try (var files = Files.list(work)) {
            for (Path file : (Iterable<Path>) files::iterator) {
                Files.delete(file);
            }
        }
        Files.delete(work);
        System.out.println("Wrote the fixture CA and server certificate to " + directory);
    }

    private static void keytool(String... args) throws IOException, InterruptedException {
        List<String> command = new ArrayList<>();
        command.add(Path.of(System.getProperty("java.home"), "bin", "keytool").toString());
        command.addAll(List.of(args));
        Process process = new ProcessBuilder(command).inheritIO().start();
        if (process.waitFor() != 0) {
            throw new IOException("keytool " + args[0] + " failed with exit code " + process.exitValue());
        }
    }
}
