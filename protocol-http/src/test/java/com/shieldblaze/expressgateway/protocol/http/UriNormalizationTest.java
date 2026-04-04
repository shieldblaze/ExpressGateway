/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.common.utils.AvailablePortUtil;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupTask;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.CsvSource;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for BUG-12: RFC 3986 Section 5.2.4 dot-segment removal on request URIs.
 *
 * <p>Covers three layers:
 * <ul>
 *   <li>Unit tests for {@link Http11ServerInboundHandler#removeDotSegments(String)}
 *       verifying the RFC 3986 Section 5.2.4 algorithm against reference examples.</li>
 *   <li>Unit tests for {@link Http11ServerInboundHandler#escapesRoot(String)} and
 *       {@link Http11ServerInboundHandler#normalizeUri(String)} verifying traversal
 *       detection and the combined normalize-or-reject logic.</li>
 *   <li>Integration tests verifying the proxy returns 400 for path-traversal
 *       URIs and 200 for URIs with dot segments that stay within root.</li>
 * </ul>
 */
class UriNormalizationTest {

    // ──────────────────────────────────────────────────────────────────────
    // Unit tests — removeDotSegments()
    // RFC 3986 Section 5.4 reference examples (path portion only)
    // ──────────────────────────────────────────────────────────────────────

    @ParameterizedTest(name = "removeDotSegments(\"{0}\") == \"{1}\"")
    @CsvSource({
            // Simple identity cases
            "/,                     /",
            "/a,                    /a",
            "/a/b/c,                /a/b/c",

            // Single dot removal (current directory)
            "/a/./b,                /a/b",
            "/a/./b/./c,            /a/b/c",
            "/./a,                  /a",

            // Double dot removal (parent directory)
            "/a/b/../c,             /a/c",
            "/a/b/c/../d,           /a/b/d",
            "/a/b/../../../c,       /c",

            // RFC 3986 Section 5.4 reference resolution examples (path only)
            "/a/b/c/./../../g,      /a/g",

            // Trailing dot segments
            "/a/b/.,                /a/b/",
            "/a/b/..,               /a/",

            // Root-level operations that stay within root
            "/a/..,                 /",

            // Leading relative segments (stripped per 2A)
            "../a,                  a",
            "./a,                   a",

            // Mixed
            "/a/b/c/../../d/e/../f, /a/d/f",
    })
    void removeDotSegments(String input, String expected) {
        assertEquals(expected, UriNormalizer.removeDotSegments(input));
    }

    /**
     * The RFC 3986 algorithm absorbs ".." at root level — "/../../etc/passwd"
     * normalizes to "/etc/passwd". This is correct per the RFC. The root-escape
     * detection happens in {@code escapesRoot()}, not in the algorithm itself.
     */
    @Test
    void removeDotSegments_absorbsExcessDotDotAtRoot() {
        // RFC 3986 Section 5.2.4 absorbs ".." at root
        assertEquals("/etc/passwd",
                UriNormalizer.removeDotSegments("/../../etc/passwd"));
        assertEquals("/etc/shadow",
                UriNormalizer.removeDotSegments("/a/../../../../etc/shadow"));
        assertEquals("/a",
                UriNormalizer.removeDotSegments("/../a"));
    }

    // ──────────────────────────────────────────────────────────────────────
    // Unit tests — escapesRoot()
    // ──────────────────────────────────────────────────────────────────────

    @Test
    void escapesRoot_detectsSimpleTraversal() {
        assertTrue(UriNormalizer.escapesRoot("/../../etc/passwd"));
    }

    @Test
    void escapesRoot_detectsDeepTraversal() {
        assertTrue(UriNormalizer.escapesRoot("/a/../../../../etc/shadow"));
    }

    @Test
    void escapesRoot_detectsSingleParentFromRoot() {
        assertTrue(UriNormalizer.escapesRoot("/../"));
        assertTrue(UriNormalizer.escapesRoot("/../a"));
    }

    @Test
    void escapesRoot_allowsTraversalWithinRoot() {
        assertFalse(UriNormalizer.escapesRoot("/a/b/../c"));
        assertFalse(UriNormalizer.escapesRoot("/a/b/c/../../d"));
    }

    @Test
    void escapesRoot_allowsCleanPaths() {
        assertFalse(UriNormalizer.escapesRoot("/"));
        assertFalse(UriNormalizer.escapesRoot("/a/b/c"));
        assertFalse(UriNormalizer.escapesRoot("/a/."));
    }

    @Test
    void escapesRoot_handlesDoubleSlashes() {
        assertFalse(UriNormalizer.escapesRoot("//a//b"));
    }

    @Test
    void escapesRoot_exactDepthBoundary() {
        // /a/.. goes to depth 1 then back to 0 — not escaping
        assertFalse(UriNormalizer.escapesRoot("/a/.."));
        // /a/../.. goes to depth 1, then 0, then -1 — escaping
        assertTrue(UriNormalizer.escapesRoot("/a/../.."));
    }

    // ──────────────────────────────────────────────────────────────────────
    // Unit tests — normalizeUri()
    // ──────────────────────────────────────────────────────────────────────

    @Test
    void normalizeUri_preservesQueryString() {
        assertEquals("/a/c?q=1&r=2",
                UriNormalizer.normalizeUri("/a/b/../c?q=1&r=2"));
    }

    @Test
    void normalizeUri_identityForCleanPath() {
        assertEquals("/api/v1/users",
                UriNormalizer.normalizeUri("/api/v1/users"));
    }

    @Test
    void normalizeUri_asteriskPassthrough() {
        assertEquals("*", UriNormalizer.normalizeUri("*"));
    }

    @Test
    void normalizeUri_nullAndEmptyPassthrough() {
        assertNull(UriNormalizer.normalizeUri(null));
        assertEquals("", UriNormalizer.normalizeUri(""));
    }

    @Test
    void normalizeUri_rejectsRootEscape() {
        // "/../../etc/passwd" — ".." exceeds depth, must return null
        assertNull(UriNormalizer.normalizeUri("/../../etc/passwd"));
    }

    @Test
    void normalizeUri_rejectsRootEscapeWithQuery() {
        assertNull(UriNormalizer.normalizeUri("/../../etc/passwd?foo=bar"));
    }

    @Test
    void normalizeUri_deepTraversalRejected() {
        // More ".." segments than real path depth
        assertNull(UriNormalizer.normalizeUri("/a/../../../../etc/shadow"));
    }

    @Test
    void normalizeUri_singleParentFromRoot() {
        assertNull(UriNormalizer.normalizeUri("/../"));
        assertNull(UriNormalizer.normalizeUri("/../a"));
    }

    @Test
    void normalizeUri_dotDoesNotEscape() {
        // Single dot at various positions — never escapes
        assertEquals("/", UriNormalizer.normalizeUri("/."));
        assertEquals("/a/", UriNormalizer.normalizeUri("/a/."));
    }

    @Test
    void normalizeUri_emptyQueryString() {
        assertEquals("/a/b?",
                UriNormalizer.normalizeUri("/a/b?"));
    }

    @Test
    void normalizeUri_withinRootTraversalNormalized() {
        // /a/b/../c has ".." within depth — should normalize to /a/c
        String result = UriNormalizer.normalizeUri("/a/b/../c");
        assertNotNull(result, "Within-root traversal should not be rejected");
        assertEquals("/a/c", result);
    }

    @Test
    void normalizeUri_rfcExampleWithQuery() {
        assertEquals("/a/g?y",
                UriNormalizer.normalizeUri("/a/b/c/./../../g?y"));
    }

    // ──────────────────────────────────────────────────────────────────────
    // Unit tests — SEC-05: Double-encoded dot rejection
    // ──────────────────────────────────────────────────────────────────────

    @Test
    void normalizeUri_rejectsDoubleEncodedDotTraversal() {
        // %252e decodes to %2e, which decodes to '.' — double-encoded traversal
        assertNull(UriNormalizer.normalizeUri("/%252e%252e/etc/passwd"),
                "Double-encoded dot traversal %252e%252e must be rejected");
    }

    @Test
    void normalizeUri_rejectsDoubleEncodedDotMixedCase() {
        // Mixed case: %252E (uppercase E) must also be caught
        assertNull(UriNormalizer.normalizeUri("/%252E%252e/etc/passwd"),
                "Mixed-case double-encoded dot %252E%252e must be rejected");
        assertNull(UriNormalizer.normalizeUri("/%252e%252E/etc/passwd"),
                "Mixed-case double-encoded dot %252e%252E must be rejected");
        assertNull(UriNormalizer.normalizeUri("/%252E%252E/etc/passwd"),
                "Uppercase double-encoded dot %252E%252E must be rejected");
    }

    @Test
    void normalizeUri_rejectsDoubleEncodedDotDeepInPath() {
        // Double-encoded dot buried deep in the path
        assertNull(UriNormalizer.normalizeUri("/a/b/%252e%252e/%252e%252e/etc/passwd"),
                "Double-encoded dots deep in path must be rejected");
    }

    @Test
    void normalizeUri_rejectsDoubleEncodedDotWithQuery() {
        // Double-encoded dot in path portion, with a query string
        assertNull(UriNormalizer.normalizeUri("/%252e%252e/etc/passwd?foo=bar"),
                "Double-encoded dot traversal with query string must be rejected");
    }

    @Test
    void containsDoubleEncodedDot_directUnitTests() {
        assertTrue(UriNormalizer.containsDoubleEncodedDot("/%252e%252e/"));
        assertTrue(UriNormalizer.containsDoubleEncodedDot("/foo/%252E/bar"));
        assertFalse(UriNormalizer.containsDoubleEncodedDot("/foo/bar"));
        assertFalse(UriNormalizer.containsDoubleEncodedDot("/%2e%2e/"));
        // %25 followed by something other than 2e should not trigger
        assertFalse(UriNormalizer.containsDoubleEncodedDot("/%2541/test"));
    }

    // ──────────────────────────────────────────────────────────────────────
    // Unit tests — SEC-06: Null byte rejection
    // ──────────────────────────────────────────────────────────────────────

    @Test
    void normalizeUri_rejectsNullByteInPath() {
        // %00 in path — null byte can cause string truncation in C backends
        assertNull(UriNormalizer.normalizeUri("/etc/passwd%00.jpg"),
                "Null byte %00 in path must be rejected");
    }

    @Test
    void normalizeUri_rejectsNullByteAtStart() {
        assertNull(UriNormalizer.normalizeUri("/%00/secret"),
                "Null byte %00 at start of path must be rejected");
    }

    @Test
    void normalizeUri_rejectsLiteralNullByteInPath() {
        // Literal null character (unlikely from HTTP parsing but defense in depth)
        assertNull(UriNormalizer.normalizeUri("/etc/passwd\0.jpg"),
                "Literal null byte in path must be rejected");
    }

    @Test
    void normalizeUri_allowsPercentEncodedPercentInQueryString() {
        // %25 in query string is legitimate (e.g., "100%25" means "100%")
        // and must not be rejected. Only the path is checked.
        assertEquals("/search?q=100%25",
                UriNormalizer.normalizeUri("/search?q=100%25"));
    }

    @Test
    void normalizeUri_allowsNullByteInQueryString() {
        // %00 in query string should not trigger rejection (only path is checked).
        // While unusual, query string content is backend-interpreted.
        assertEquals("/search?q=%00",
                UriNormalizer.normalizeUri("/search?q=%00"));
    }

    @Test
    void normalizeUri_cleanPathWithPercentEncodingStillWorks() {
        // A path with normal percent-encoding that is not a dot or null
        assertEquals("/path/%20with%20spaces",
                UriNormalizer.normalizeUri("/path/%20with%20spaces"));
    }

    // ──────────────────────────────────────────────────────────────────────
    // Integration test — proxy returns 400 for path traversal
    // ──────────────────────────────────────────────────────────────────────

    private static int loadBalancerPort;
    private static HTTPLoadBalancer httpLoadBalancer;
    private static HttpServer httpServer;

    @BeforeAll
    static void setup() throws Exception {
        httpServer = new HttpServer(false);
        httpServer.start();
        httpServer.START_FUTURE.get();

        loadBalancerPort = AvailablePortUtil.getTcpPort();

        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withConfigurationContext(ConfigurationContext.DEFAULT)
                .withBindAddress(new InetSocketAddress("127.0.0.1", loadBalancerPort))
                .withL4FrontListener(new TCPListener())
                .build();

        httpLoadBalancer.mappedCluster("localhost:" + loadBalancerPort, cluster);

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("127.0.0.1", httpServer.port()))
                .build();

        L4FrontListenerStartupTask startupTask = httpLoadBalancer.start();
        startupTask.future().join();
        assertTrue(startupTask.isSuccess());
    }

    @AfterAll
    static void shutdown() throws Exception {
        httpLoadBalancer.shutdown().future().get();
        httpServer.shutdown();
        httpServer.SHUTDOWN_FUTURE.get();
    }

    /**
     * BUG-12: A request whose URI escapes the document root MUST be
     * rejected with 400 Bad Request.
     */
    @Test
    void pathTraversal_returns400() throws Exception {
        String rawRequest = "GET /../../etc/passwd HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("400"),
                "Path traversal URI should return 400 but got: " + response);
    }

    /**
     * A request with dot segments that resolve to a valid path within root
     * should be normalized and forwarded successfully (200 from echo backend).
     */
    @Test
    void dotSegmentsWithinRoot_normalizedAndForwarded() throws Exception {
        String rawRequest = "GET /a/b/../c HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("200"),
                "Normalized URI within root should be forwarded, but got: " + response);
    }

    /**
     * SEC-05: Double-encoded dot traversal must return 400.
     */
    @Test
    void doubleEncodedDotTraversal_returns400() throws Exception {
        String rawRequest = "GET /%252e%252e/etc/passwd HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("400"),
                "Double-encoded dot traversal should return 400 but got: " + response);
    }

    /**
     * SEC-06: Null byte in URI path must return 400.
     */
    @Test
    void nullByteInPath_returns400() throws Exception {
        String rawRequest = "GET /etc/passwd%00.jpg HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("400"),
                "Null byte in URI path should return 400 but got: " + response);
    }

    /**
     * A request with %25 in the query string (not path) should be forwarded
     * normally — only the path portion is checked for double-encoded dots.
     */
    @Test
    void percentInQueryString_notRejected() throws Exception {
        String rawRequest = "GET /search?q=100%25 HTTP/1.1\r\n" +
                "Host: localhost:" + loadBalancerPort + "\r\n" +
                "\r\n";

        String response = sendRawRequest(rawRequest);
        assertTrue(response.contains("200"),
                "Percent in query string should not be rejected, but got: " + response);
    }

    private String sendRawRequest(String rawRequest) throws Exception {
        try (Socket socket = new Socket("127.0.0.1", loadBalancerPort)) {
            socket.setSoTimeout(5000);

            OutputStream out = socket.getOutputStream();
            out.write(rawRequest.getBytes(StandardCharsets.UTF_8));
            out.flush();

            try (BufferedReader reader = new BufferedReader(
                    new InputStreamReader(socket.getInputStream(), StandardCharsets.UTF_8))) {

                String statusLine = reader.readLine();
                if (statusLine == null) {
                    return "";
                }
                return statusLine;
            }
        }
    }
}
