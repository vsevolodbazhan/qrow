package io.qrow.fixture;

import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpsConfigurator;
import com.sun.net.httpserver.HttpsServer;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.math.BigInteger;
import java.net.InetSocketAddress;
import java.net.URLDecoder;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.SecureRandom;
import java.security.Signature;
import java.security.interfaces.RSAPublicKey;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Base64;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.Executors;
import java.util.regex.Pattern;
import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;

/**
 * Mock OpenID Connect provider over HTTPS for end-to-end tests. Never shipped with Qrow.
 *
 * <p>Usage: {@code Oidc <port> <pkcs12-keystore> <keystore-password> <jwks-output-file> [bind-host]}.
 * The issuer is {@code https://127.0.0.1:<port>}. The only client is the public client
 * {@code qrow-desktop} with loopback redirect URIs {@code http://127.0.0.1:<any port>/callback}.
 * The fake browser of a test chooses the user and token behaviour with {@code fixture_*} query
 * parameters of the authorization request.
 */
public final class Oidc {
    // The disposable Trino fixture uses a confidential, server-owned client.
    // The existing Kyuubi fixture keeps its public PKCE client.
    static final String TRINO_ORIGIN = System.getenv("QROW_FIXTURE_TRINO_ORIGIN");
    static final String CLIENT = TRINO_ORIGIN == null ? "qrow-desktop" : "trino";
    static final String AUDIENCE = TRINO_ORIGIN == null ? "kyuubi" : "trino";
    static final String KID = "fixture-1";
    static final List<String> SCOPES = List.of("openid", "profile", "email", "kyuubi", "offline_access");
    static final Pattern REDIRECT = Pattern.compile("http://127\\.0\\.0\\.1:([1-9][0-9]{0,4})/callback");
    static final long CODE_SECONDS = 60;
    static final long ID_TOKEN_SECONDS = 300;

    record User(String name, String subject, String displayName, String email, List<String> accounts) {}

    static final Map<String, User> USERS = Map.of(
        "alice", new User("alice", "8d2f1a7e-0000-4000-8000-00000000a11c", "Alice Fixture", "alice@qrow.test",
            List.of("qrow")),
        "bob", new User("bob", "8d2f1a7e-0000-4000-8000-000000000b0b", "Bob Fixture", "bob@qrow.test",
            List.of("qrow")),
        "mallory", new User("mallory", "8d2f1a7e-0000-4000-8000-000000000bad",
            "Mallory Fixture", "mallory@qrow.test", List.of()));

    record Code(String clientId, String redirectUri, String challenge, String nonce, Set<String> scope, User user,
                long ttl, boolean refresh, long expires) {}

    /** One sign-in. Every refresh token of a family descends from one authorization code. */
    static final class Family {
        final User user;
        final Set<String> scope;
        final long ttl;
        String current;
        boolean revoked;

        Family(User user, Set<String> scope, long ttl) {
            this.user = user;
            this.scope = scope;
            this.ttl = ttl;
        }
    }

    final String issuer;
    final KeyPair keys = generateKeys();
    final SecureRandom random = new SecureRandom();
    final Map<String, Code> codes = new HashMap<>();
    /** Every refresh token issued, current or rotated, to its family. */
    final Map<String, Family> refreshTokens = new HashMap<>();
    final Map<String, Long> stats = new LinkedHashMap<>();

    Oidc(String issuer) {
        this.issuer = issuer;
        resetStats();
    }

    public static void main(String[] args) throws Exception {
        int port = Integer.parseInt(args[0]);
        String host = args.length > 4 ? args[4] : "127.0.0.1";
        Oidc provider = new Oidc("https://127.0.0.1:" + port);
        writeAtomically(Path.of(args[3]), provider.jwks());
        HttpsServer server = HttpsServer.create(new InetSocketAddress(host, port), 64);
        server.setHttpsConfigurator(new HttpsConfigurator(sslContext(args[1], args[2])));
        server.setExecutor(Executors.newFixedThreadPool(8));
        server.createContext("/", provider::handle);
        server.start();
        System.out.println("OIDC provider " + provider.issuer + " listens on " + host + ":" + port);
        Runtime.getRuntime().addShutdownHook(new Thread(() -> server.stop(0)));
        new CountDownLatch(1).await();
    }

    static SSLContext sslContext(String keystore, String password) throws Exception {
        KeyStore store = KeyStore.getInstance("PKCS12");
        try (InputStream input = new FileInputStream(keystore)) {
            store.load(input, password.toCharArray());
        }
        KeyManagerFactory keys = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
        keys.init(store, password.toCharArray());
        SSLContext context = SSLContext.getInstance("TLS");
        context.init(keys.getKeyManagers(), null, null);
        return context;
    }

    static KeyPair generateKeys() {
        try {
            KeyPairGenerator generator = KeyPairGenerator.getInstance("RSA");
            generator.initialize(2048);
            return generator.generateKeyPair();
        } catch (java.security.GeneralSecurityException error) {
            throw new IllegalStateException(error);
        }
    }

    static void writeAtomically(Path path, String text) throws IOException {
        Path absolute = path.toAbsolutePath();
        Files.createDirectories(absolute.getParent());
        Path temporary = Files.createTempFile(absolute.getParent(), ".jwks-", ".tmp");
        Files.writeString(temporary, text);
        temporary.toFile().setReadable(true, false);
        Files.move(temporary, absolute, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
    }

    // ---- HTTP plumbing ----------------------------------------------------------------------

    record Response(int status, String type, String body, Map<String, String> headers) {
        static Response json(int status, String body) {
            return new Response(status, "application/json", body, Map.of("Cache-Control", "no-store"));
        }

        static Response error(int status, String error, String description) {
            return json(status, object("error", error, "error_description", description));
        }

        static Response page(int status, String message) {
            return new Response(status, "text/plain; charset=utf-8", message + "\n", Map.of());
        }

        static Response redirect(String location) {
            return new Response(302, "text/plain; charset=utf-8", "", Map.of("Location", location));
        }
    }

    void handle(HttpExchange exchange) throws IOException {
        Response response;
        try {
            response = route(exchange);
        } catch (RuntimeException error) {
            response = Response.error(500, "server_error", error.getClass().getSimpleName());
        }
        byte[] body = response.body().getBytes(StandardCharsets.UTF_8);
        exchange.getResponseHeaders().set("Content-Type", response.type());
        response.headers().forEach(exchange.getResponseHeaders()::set);
        exchange.sendResponseHeaders(response.status(), body.length == 0 ? -1 : body.length);
        if (body.length > 0) {
            try (OutputStream output = exchange.getResponseBody()) {
                output.write(body);
            }
        }
        exchange.close();
        // The path only: query strings and bodies can hold codes and tokens.
        System.out.println(exchange.getRequestMethod() + " " + exchange.getRequestURI().getRawPath() + " "
            + response.status());
    }

    Response route(HttpExchange exchange) throws IOException {
        String method = exchange.getRequestMethod();
        String path = exchange.getRequestURI().getRawPath();
        Map<String, String> query = form(exchange.getRequestURI().getRawQuery());
        boolean get = method.equals("GET");
        boolean post = method.equals("POST");
        switch (path) {
            case "/.well-known/openid-configuration":
                return get ? Response.json(200, discovery()) : notAllowed();
            case "/jwks":
                return get ? Response.json(200, jwks()) : notAllowed();
            case "/authorize":
                return get ? authorize(query) : notAllowed();
            case "/token":
                if (!post) {
                    return notAllowed();
                }
                String type = exchange.getRequestHeaders().getFirst("Content-Type");
                if (type == null || !type.toLowerCase().startsWith("application/x-www-form-urlencoded")) {
                    return Response.error(400, "invalid_request", "Use application/x-www-form-urlencoded");
                }
                Map<String, String> tokenForm = form(new String(exchange.getRequestBody().readAllBytes(), StandardCharsets.UTF_8));
                if (TRINO_ORIGIN != null) {
                    String authorization = exchange.getRequestHeaders().getFirst("Authorization");
                    String expected = "Basic " + Base64.getEncoder().encodeToString(("trino:synthetic-trino-client-secret").getBytes(StandardCharsets.UTF_8));
                    boolean secretForm = "synthetic-trino-client-secret".equals(tokenForm.get("client_secret"));
                    if (!expected.equals(authorization) && !secretForm) {
                        return Response.error(401, "invalid_client", "The confidential client secret is required");
                    }
                    tokenForm.put("client_id", CLIENT);
                }
                return token(tokenForm);
            case "/fixture/revoke":
                return post ? revoke(query.get("user")) : notAllowed();
            case "/fixture/stats":
                return get ? Response.json(200, stats()) : notAllowed();
            case "/fixture/reset-stats":
                if (!post) {
                    return notAllowed();
                }
                resetStats();
                return Response.json(200, stats());
            default:
                return Response.page(404, "Not found");
        }
    }

    static Response notAllowed() {
        return Response.page(405, "Method not allowed");
    }

    static Map<String, String> form(String raw) {
        Map<String, String> values = new HashMap<>();
        if (raw == null || raw.isEmpty()) {
            return values;
        }
        for (String pair : raw.split("&")) {
            int equals = pair.indexOf('=');
            String key = equals < 0 ? pair : pair.substring(0, equals);
            String value = equals < 0 ? "" : pair.substring(equals + 1);
            // A repeated parameter is invalid (RFC 6749 section 3.1); an empty name marks it.
            String name = URLDecoder.decode(key, StandardCharsets.UTF_8);
            if (values.containsKey(name)) {
                values.put("", name);
            }
            values.put(name, URLDecoder.decode(value, StandardCharsets.UTF_8));
        }
        return values;
    }

    static String encode(String value) {
        return URLEncoder.encode(value, StandardCharsets.UTF_8).replace("+", "%20");
    }

    // ---- Discovery and keys -----------------------------------------------------------------

    String discovery() {
        return "{" + String.join(",",
            field("issuer", issuer),
            field("authorization_endpoint", issuer + "/authorize"),
            field("token_endpoint", issuer + "/token"),
            field("jwks_uri", issuer + "/jwks"),
            raw("response_types_supported", array(List.of("code"))),
            raw("subject_types_supported", array(List.of("public"))),
            raw("id_token_signing_alg_values_supported", array(List.of("RS256"))),
            raw("code_challenge_methods_supported", array(List.of("S256"))),
            raw("grant_types_supported", array(List.of("authorization_code", "refresh_token"))),
            raw("scopes_supported", array(SCOPES)),
            raw("token_endpoint_auth_methods_supported", array(TRINO_ORIGIN == null
                ? List.of("none") : List.of("client_secret_basic", "client_secret_post")))) + "}";
    }

    String jwks() {
        RSAPublicKey key = (RSAPublicKey) keys.getPublic();
        String jwk = "{" + String.join(",", field("kty", "RSA"), field("kid", KID), field("use", "sig"),
            field("alg", "RS256"), field("n", unsigned(key.getModulus())),
            field("e", unsigned(key.getPublicExponent()))) + "}";
        return "{\"keys\":[" + jwk + "]}";
    }

    static String unsigned(BigInteger value) {
        byte[] bytes = value.toByteArray();
        if (bytes.length > 1 && bytes[0] == 0) {
            bytes = Arrays.copyOfRange(bytes, 1, bytes.length);
        }
        return base64(bytes);
    }

    static String base64(byte[] bytes) {
        return Base64.getUrlEncoder().withoutPadding().encodeToString(bytes);
    }

    String randomToken() {
        byte[] bytes = new byte[32];
        random.nextBytes(bytes);
        return base64(bytes);
    }

    // ---- Authorization endpoint -------------------------------------------------------------

    Response authorize(Map<String, String> query) {
        if (query.containsKey("")) {
            return Response.page(400, "Repeated parameter: " + query.get(""));
        }
        if (!CLIENT.equals(query.get("client_id"))) {
            return Response.page(400, "Unknown client_id");
        }
        String redirect = query.get("redirect_uri");
        if (redirect == null || !loopbackRedirect(redirect)) {
            return Response.page(400, "redirect_uri must be http://127.0.0.1:<port>/callback");
        }
        String state = query.get("state");
        String problem = authorizationProblem(query);
        if (problem != null) {
            return redirectError(redirect, "invalid_request", problem, state);
        }
        if ("1".equals(query.get("fixture_deny"))) {
            return redirectError(redirect, "access_denied", "The fixture user denied access", state);
        }
        User user = USERS.get(query.getOrDefault("fixture_user", "alice"));
        long ttl;
        try {
            ttl = Long.parseLong(query.getOrDefault("fixture_access_ttl", "300"));
        } catch (NumberFormatException error) {
            ttl = -1;
        }
        String refresh = query.getOrDefault("fixture_refresh", "rotate");
        if (user == null || ttl < 1 || !(refresh.equals("rotate") || refresh.equals("none"))) {
            return redirectError(redirect, "invalid_request", "Invalid fixture parameter", state);
        }
        String code = randomToken();
        synchronized (this) {
            codes.put(code, new Code(CLIENT, redirect, query.get("code_challenge"), query.get("nonce"),
                scopes(query.get("scope")), user, ttl, refresh.equals("rotate"), now() + CODE_SECONDS));
        }
        return Response.redirect(redirect + "?code=" + encode(code) + "&state=" + encode(state)
            + "&iss=" + encode(issuer));
    }

    static boolean loopbackRedirect(String uri) {
        if (TRINO_ORIGIN != null) { return uri.equals(TRINO_ORIGIN + "/oauth2/callback"); }
        var match = REDIRECT.matcher(uri);
        return match.matches() && Integer.parseInt(match.group(1)) <= 65535;
    }

    static String authorizationProblem(Map<String, String> query) {
        if (!"code".equals(query.get("response_type"))) {
            return "response_type must be code";
        }
        String scope = query.get("scope");
        if (scope == null || !scopes(scope).contains("openid")) {
            return "scope must contain openid";
        }
        if (!SCOPES.containsAll(scopes(scope))) {
            return "Unknown scope";
        }
        for (String name : TRINO_ORIGIN == null ? List.of("state", "nonce", "code_challenge") : List.of("state")) {
            if (query.getOrDefault(name, "").isEmpty()) {
                return name + " is required";
            }
        }
        if (TRINO_ORIGIN == null && !"S256".equals(query.get("code_challenge_method"))) {
            return "code_challenge_method must be S256";
        }
        return null;
    }

    static Response redirectError(String redirect, String error, String description, String state) {
        String location = redirect + "?error=" + error + "&error_description=" + encode(description);
        if (state != null) {
            location += "&state=" + encode(state);
        }
        return Response.redirect(location);
    }

    static Set<String> scopes(String scope) {
        Set<String> values = new LinkedHashSet<>();
        if (scope != null) {
            for (String value : scope.trim().split(" +")) {
                if (!value.isEmpty()) {
                    values.add(value);
                }
            }
        }
        return values;
    }

    // ---- Token endpoint ---------------------------------------------------------------------

    Response token(Map<String, String> form) {
        if (form.containsKey("")) {
            return Response.error(400, "invalid_request", "Repeated parameter");
        }
        String grant = form.get("grant_type");
        if (grant == null) {
            return Response.error(400, "invalid_request", "grant_type is required");
        }
        if (form.get("client_id") == null) {
            return Response.error(400, "invalid_request", "client_id is required");
        }
        if (!CLIENT.equals(form.get("client_id"))) {
            return Response.error(401, "invalid_client", "Unknown client");
        }
        switch (grant) {
            case "authorization_code":
                return authorizationCode(form);
            case "refresh_token":
                return refreshToken(form);
            default:
                return Response.error(400, "unsupported_grant_type", "Unsupported grant_type");
        }
    }

    synchronized Response authorizationCode(Map<String, String> form) {
        String value = form.get("code");
        String verifier = form.get("code_verifier");
        String redirect = form.get("redirect_uri");
        if (value == null || (TRINO_ORIGIN == null && verifier == null) || redirect == null) {
            return Response.error(400, "invalid_request", "code, redirect_uri, and code_verifier are required");
        }
        // Single use: a failed redemption also consumes the code.
        Code code = codes.remove(value);
        if (code == null || code.expires() < now()) {
            return Response.error(400, "invalid_grant", "Unknown, used, or expired code");
        }
        if (!code.redirectUri().equals(redirect) || !code.clientId().equals(form.get("client_id"))) {
            return Response.error(400, "invalid_grant", "redirect_uri or client_id does not match");
        }
        if (code.challenge() != null && (verifier == null || !verifier.matches("[A-Za-z0-9._~-]{43,128}") || !challenge(verifier).equals(code.challenge()))) {
            return Response.error(400, "invalid_grant", "PKCE verification failed");
        }
        Family family = new Family(code.user(), code.scope(), code.ttl());
        String refresh = code.refresh() ? rotate(family) : null;
        count("authorization_code");
        return Response.json(200, tokens(family, family.scope, refresh, idToken(code)));
    }

    synchronized Response refreshToken(Map<String, String> form) {
        String value = form.get("refresh_token");
        if (value == null) {
            return Response.error(400, "invalid_request", "refresh_token is required");
        }
        Family family = refreshTokens.get(value);
        if (family == null || family.revoked) {
            return Response.error(400, "invalid_grant", "Unknown or revoked refresh token");
        }
        if (!value.equals(family.current)) {
            // Reuse of a rotated token: someone else may hold the family. Revoke all of it.
            family.revoked = true;
            return Response.error(400, "invalid_grant", "Refresh token reuse detected; the sign-in is revoked");
        }
        Set<String> scope = family.scope;
        if (form.containsKey("scope")) {
            scope = scopes(form.get("scope"));
            if (scope.isEmpty() || !family.scope.containsAll(scope)) {
                return Response.error(400, "invalid_scope", "scope must be a subset of the original scope");
            }
        }
        String refresh = rotate(family);
        count("refresh_token");
        return Response.json(200, tokens(family, scope, refresh,
            TRINO_ORIGIN == null ? null : idToken(family.user, null)));
    }

    String rotate(Family family) {
        String refresh = randomToken();
        family.current = refresh;
        refreshTokens.put(refresh, family);
        return refresh;
    }

    static String challenge(String verifier) {
        try {
            return base64(MessageDigest.getInstance("SHA-256").digest(verifier.getBytes(StandardCharsets.US_ASCII)));
        } catch (java.security.NoSuchAlgorithmException error) {
            throw new IllegalStateException(error);
        }
    }

    String tokens(Family family, Set<String> scope, String refresh, String idToken) {
        long issued = now();
        String scopeText = String.join(" ", scope);
        String access = jwt(TRINO_ORIGIN == null ? "at+jwt" : "JWT", String.join(",",
            field("iss", issuer), field("sub", family.user.subject()), field("aud", AUDIENCE),
            field("azp", CLIENT), raw("iat", Long.toString(issued)), raw("exp", Long.toString(issued + family.ttl)),
            field("scope", scopeText), field("preferred_username", family.user.name()), field("jti", randomToken()),
            raw("qrow_accounts", array(family.user.accounts()))));
        List<String> fields = new ArrayList<>(List.of(field("access_token", access), field("token_type", "Bearer"),
            raw("expires_in", Long.toString(family.ttl)), field("scope", scopeText)));
        if (idToken != null) {
            fields.add(field("id_token", idToken));
        }
        if (refresh != null) {
            fields.add(field("refresh_token", refresh));
        }
        return "{" + String.join(",", fields) + "}";
    }

    String idToken(Code code) {
        return idToken(code.user(), code.nonce());
    }

    String idToken(User user, String nonce) {
        long issued = now();
        return jwt("JWT", String.join(",",
            field("iss", issuer), field("sub", user.subject()), field("aud", CLIENT), field("azp", CLIENT),
            raw("iat", Long.toString(issued)), raw("exp", Long.toString(issued + ID_TOKEN_SECONDS)),
            field("nonce", nonce == null ? "" : nonce), field("name", user.displayName()), field("email", user.email()),
            field("preferred_username", user.name())));
    }

    String jwt(String type, String claims) {
        String header = "{" + String.join(",", field("alg", "RS256"), field("typ", type), field("kid", KID)) + "}";
        String input = base64(header.getBytes(StandardCharsets.UTF_8)) + "."
            + base64(("{" + claims + "}").getBytes(StandardCharsets.UTF_8));
        try {
            Signature signature = Signature.getInstance("SHA256withRSA");
            signature.initSign(keys.getPrivate());
            signature.update(input.getBytes(StandardCharsets.US_ASCII));
            return input + "." + base64(signature.sign());
        } catch (java.security.GeneralSecurityException error) {
            throw new IllegalStateException(error);
        }
    }

    // ---- Test control -----------------------------------------------------------------------

    synchronized Response revoke(String name) {
        User user = USERS.get(name);
        if (user == null) {
            return Response.error(400, "invalid_request", "Unknown user");
        }
        Set<Family> families = new LinkedHashSet<>();
        for (Family family : refreshTokens.values()) {
            if (family.user.equals(user) && !family.revoked) {
                family.revoked = true;
                families.add(family);
            }
        }
        return Response.json(200, "{" + raw("revoked", Integer.toString(families.size())) + "}");
    }

    synchronized void count(String grant) {
        stats.merge(grant, 1L, Long::sum);
    }

    synchronized void resetStats() {
        stats.put("authorization_code", 0L);
        stats.put("refresh_token", 0L);
    }

    synchronized String stats() {
        List<String> fields = new ArrayList<>();
        stats.forEach((key, value) -> fields.add(raw(key, Long.toString(value))));
        return "{" + String.join(",", fields) + "}";
    }

    static long now() {
        return System.currentTimeMillis() / 1000;
    }

    // ---- Minimal JSON writer ----------------------------------------------------------------

    static String field(String key, String value) {
        return quote(key) + ":" + quote(value);
    }

    static String raw(String key, String json) {
        return quote(key) + ":" + json;
    }

    static String object(String... pairs) {
        List<String> fields = new ArrayList<>();
        for (int index = 0; index < pairs.length; index += 2) {
            fields.add(field(pairs[index], pairs[index + 1]));
        }
        return "{" + String.join(",", fields) + "}";
    }

    static String array(List<String> values) {
        List<String> quoted = new ArrayList<>();
        values.forEach(value -> quoted.add(quote(value)));
        return "[" + String.join(",", quoted) + "]";
    }

    static String quote(String value) {
        StringBuilder text = new StringBuilder("\"");
        for (char character : value.toCharArray()) {
            switch (character) {
                case '"' -> text.append("\\\"");
                case '\\' -> text.append("\\\\");
                default -> {
                    if (character < 0x20) {
                        text.append(String.format("\\u%04x", (int) character));
                    } else {
                        text.append(character);
                    }
                }
            }
        }
        return text.append('"').toString();
    }
}
