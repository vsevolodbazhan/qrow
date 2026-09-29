package io.qrow.fixture;

import com.unboundid.ldap.listener.InMemoryDirectoryServer;
import com.unboundid.ldap.listener.InMemoryDirectoryServerConfig;
import com.unboundid.ldap.listener.InMemoryListenerConfig;
import java.net.InetAddress;
import java.util.concurrent.CountDownLatch;

/** Real loopback LDAP listener for the native macOS test fixture. */
public final class Ldap {
    public static void main(String[] args) throws Exception {
        InMemoryDirectoryServerConfig config = new InMemoryDirectoryServerConfig("dc=qrow,dc=test");
        config.setListenerConfigs(InMemoryListenerConfig.createLDAPConfig(
            "fixture", InetAddress.getByName("127.0.0.1"), Integer.parseInt(args[0]), null));
        InMemoryDirectoryServer server = new InMemoryDirectoryServer(config);
        server.add("dn: dc=qrow,dc=test", "objectClass: domain", "dc: qrow");
        server.importFromLDIF(false, args[1]);
        server.startListening();
        Runtime.getRuntime().addShutdownHook(new Thread(() -> server.shutDown(true)));
        new CountDownLatch(1).await();
    }
}
