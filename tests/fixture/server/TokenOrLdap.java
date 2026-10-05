package io.qrow.fixture;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.io.IOException;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.FileTime;
import java.security.KeyFactory;
import java.security.PublicKey;
import java.security.Signature;
import java.security.spec.RSAPublicKeySpec;
import java.util.Base64;
import java.util.HashMap;
import java.util.Map;
import java.util.regex.Pattern;
import javax.security.sasl.AuthenticationException;
import org.apache.kyuubi.config.KyuubiConf;
import org.apache.kyuubi.service.authentication.LdapAuthenticationProviderImpl;
import org.apache.kyuubi.service.authentication.PasswdAuthenticationProvider;
import org.apache.kyuubi.service.authentication.ldap.LdapSearchFactory;

/**
 * Kyuubi password authenticator of the fixture: an OIDC access token or the LDAP password.
 * Never shipped with Qrow.
 *
 * <p>Kyuubi creates it for {@code kyuubi.authentication CUSTOM} through its {@code KyuubiConf}
 * constructor. A password that is a JWT with an {@code alg} header is an access token. It must be
 * signed by a key of the provider JWKS file ({@code -Dqrow.oidc.jwks}), come from the issuer
 * {@code -Dqrow.oidc.issuer}, have the audience {@code kyuubi} and the scope {@code kyuubi}, be
 * unexpired, and list the requested database user in {@code qrow_accounts}. Any other password goes
 * to Kyuubi's LDAP authenticator with the {@code kyuubi.authentication.ldap.*} settings.
 */
public final class TokenOrLdap implements PasswdAuthenticationProvider {
    private static final Pattern JWT = Pattern.compile("[A-Za-z0-9_-]+\\.[A-Za-z0-9_-]+\\.[A-Za-z0-9_-]+");
    private static final ObjectMapper JSON = new ObjectMapper();
    private static final Object LOCK = new Object();
    private static Map<String, PublicKey> keys = Map.of();
    private static FileTime loaded;

    private final PasswdAuthenticationProvider ldap;

    public TokenOrLdap(KyuubiConf conf) {
        ldap = new LdapAuthenticationProviderImpl(conf, new LdapSearchFactory());
    }

    @Override
    public void authenticate(String user, String password) throws AuthenticationException {
        JsonNode header = header(password);
        if (header == null) {
            ldap.authenticate(user, password);
            return;
        }
        try {
            validate(user, password, header);
        } catch (AuthenticationException error) {
            throw error;
        } catch (Exception error) {
            // The message of a parser error can quote the token.
            throw new AuthenticationException("Access token rejected: malformed token");
        }
    }

    /** The JOSE header when the password is a JWT, otherwise null. */
    static JsonNode header(String password) {
        if (password == null || !JWT.matcher(password).matches()) {
            return null;
        }
        try {
            JsonNode header = JSON.readTree(decode(password.substring(0, password.indexOf('.'))));
            return header != null && header.isObject() && header.has("alg") ? header : null;
        } catch (IOException | IllegalArgumentException error) {
            return null;
        }
    }

    private static void validate(String user, String token, JsonNode header) throws Exception {
        if (!"RS256".equals(header.path("alg").asText())) {
            throw rejected("unsupported algorithm");
        }
        String kid = header.path("kid").asText("");
        PublicKey key = key(kid);
        if (key == null) {
            throw rejected("unknown signing key");
        }
        int signed = token.lastIndexOf('.');
        Signature signature = Signature.getInstance("SHA256withRSA");
        signature.initVerify(key);
        signature.update(token.substring(0, signed).getBytes(StandardCharsets.US_ASCII));
        if (!signature.verify(decode(token.substring(signed + 1)))) {
            throw rejected("invalid signature");
        }
        JsonNode claims = JSON.readTree(decode(token.substring(token.indexOf('.') + 1, signed)));
        if (claims == null || !claims.isObject()) {
            throw rejected("malformed claims");
        }
        String issuer = System.getProperty("qrow.oidc.issuer");
        if (issuer == null || !issuer.equals(claims.path("iss").asText(null))) {
            throw rejected("wrong issuer");
        }
        if (!contains(claims.path("aud"), "kyuubi")) {
            throw rejected("wrong audience");
        }
        long now = System.currentTimeMillis() / 1000;
        if (!claims.path("exp").canConvertToLong() || claims.path("exp").asLong() <= now) {
            throw rejected("expired");
        }
        if (claims.has("nbf") && claims.path("nbf").asLong() > now) {
            throw rejected("not yet valid");
        }
        boolean scoped = false;
        for (String scope : claims.path("scope").asText("").split(" ")) {
            scoped |= scope.equals("kyuubi");
        }
        if (!scoped) {
            throw rejected("missing scope kyuubi");
        }
        if (!contains(claims.path("qrow_accounts"), user)) {
            throw rejected("the identity cannot use database user " + user);
        }
    }

    private static boolean contains(JsonNode node, String value) {
        if (node.isTextual()) {
            return node.asText().equals(value);
        }
        if (node.isArray()) {
            for (JsonNode item : node) {
                if (item.isTextual() && item.asText().equals(value)) {
                    return true;
                }
            }
        }
        return false;
    }

    /** The key of `kid`. Reloads the JWKS file when it changed or does not know `kid`. */
    private static PublicKey key(String kid) throws Exception {
        String file = System.getProperty("qrow.oidc.jwks");
        if (file == null) {
            throw rejected("no JWKS file configured");
        }
        Path path = Path.of(file);
        synchronized (LOCK) {
            FileTime modified = Files.exists(path) ? Files.getLastModifiedTime(path) : null;
            if (modified != null && (!keys.containsKey(kid) || !modified.equals(loaded))) {
                Map<String, PublicKey> fresh = new HashMap<>();
                KeyFactory factory = KeyFactory.getInstance("RSA");
                for (JsonNode jwk : JSON.readTree(Files.readAllBytes(path)).path("keys")) {
                    if ("RSA".equals(jwk.path("kty").asText()) && jwk.hasNonNull("kid")) {
                        fresh.put(jwk.path("kid").asText(), factory.generatePublic(new RSAPublicKeySpec(
                            new BigInteger(1, decode(jwk.path("n").asText())),
                            new BigInteger(1, decode(jwk.path("e").asText())))));
                    }
                }
                keys = fresh;
                loaded = modified;
            }
            return keys.get(kid);
        }
    }

    private static byte[] decode(String value) {
        return Base64.getUrlDecoder().decode(value);
    }

    private static AuthenticationException rejected(String reason) {
        return new AuthenticationException("Access token rejected: " + reason);
    }
}
