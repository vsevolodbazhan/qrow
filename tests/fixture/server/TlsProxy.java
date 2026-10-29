package io.qrow.fixture;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Socket;
import javax.net.ssl.SSLServerSocket;
import javax.net.ssl.SSLSocket;

/**
 * TLS in front of the plain binary Thrift port of Kyuubi. Never shipped with Qrow.
 *
 * <p>Usage: {@code TlsProxy <listen-port> <target-host> <target-port> <pkcs12-keystore> <password> [bind-host]}.
 * Kyuubi cannot serve plain and TLS binary transports at the same time, so this proxy is the
 * protected transport of the fixture. It only moves bytes; SASL PLAIN runs inside the TLS stream.
 */
public final class TlsProxy {
    private TlsProxy() {}

    public static void main(String[] args) throws Exception {
        int port = Integer.parseInt(args[0]);
        String target = args[1];
        int targetPort = Integer.parseInt(args[2]);
        String host = args.length > 5 ? args[5] : "127.0.0.1";
        var context = Oidc.sslContext(args[3], args[4]);
        SSLServerSocket server = (SSLServerSocket) context.getServerSocketFactory()
            .createServerSocket(port, 64, InetAddress.getByName(host));
        server.setEnabledProtocols(new String[] {"TLSv1.3", "TLSv1.2"});
        System.out.println("TLS proxy listens on " + host + ":" + port + " for " + target + ":" + targetPort);
        Runtime.getRuntime().addShutdownHook(new Thread(() -> close(server)));
        while (!server.isClosed()) {
            SSLSocket client;
            try {
                client = (SSLSocket) server.accept();
            } catch (IOException error) {
                if (server.isClosed()) {
                    break;
                }
                System.out.println("Accept failed: " + error.getMessage());
                continue;
            }
            Thread thread = new Thread(() -> serve(client, target, targetPort), "tls-proxy-client");
            thread.setDaemon(true);
            thread.start();
        }
    }

    private static void serve(SSLSocket client, String target, int targetPort) {
        String peer = String.valueOf(client.getRemoteSocketAddress());
        try (client; Socket upstream = new Socket()) {
            client.startHandshake();
            upstream.connect(new InetSocketAddress(target, targetPort), 10_000);
            upstream.setTcpNoDelay(true);
            client.setTcpNoDelay(true);
            System.out.println("Connection from " + peer + " (" + client.getSession().getProtocol() + ")");
            Thread back = new Thread(() -> pump(upstream, client), "tls-proxy-upstream");
            back.setDaemon(true);
            back.start();
            pump(client, upstream);
            back.join();
        } catch (IOException | InterruptedException error) {
            System.out.println("Connection from " + peer + " ended: " + error.getMessage());
        }
    }

    /** Copies until either side ends, then closes both sides so the other pump also ends. */
    private static void pump(Socket from, Socket to) {
        byte[] buffer = new byte[64 * 1024];
        try {
            InputStream input = from.getInputStream();
            OutputStream output = to.getOutputStream();
            int count;
            while ((count = input.read(buffer)) >= 0) {
                output.write(buffer, 0, count);
                output.flush();
            }
        } catch (IOException ignored) {
            // The other side closed; close both below.
        } finally {
            close(from);
            close(to);
        }
    }

    private static void close(java.io.Closeable closeable) {
        try {
            closeable.close();
        } catch (IOException ignored) {
            // Already closed.
        }
    }
}
