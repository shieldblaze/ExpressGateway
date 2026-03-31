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

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceResponse;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.StickySession;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.EmptyHttpHeaders;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaders;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests the sticky session round trip: a first request gets a Set-Cookie header
 * in the response, and subsequent requests with that cookie are routed to the
 * same backend node.
 *
 * <p>This validates the full lifecycle of cookie-based session persistence as
 * implemented in {@link StickySession}:
 * <ol>
 *   <li>First request: no cookie present; load balancer selects a node via
 *       round-robin and returns a Set-Cookie header with the node's ID.</li>
 *   <li>Subsequent requests: the client sends the cookie back; the load
 *       balancer performs a binary search on the sorted node list to find
 *       the matching node and routes to it directly.</li>
 * </ol>
 *
 * <p>This test also validates the fix for BUG-003 (inverted type casts in
 * {@code StickySessionSearchComparator}). Before the fix, the binary search
 * threw {@code ClassCastException} and sticky sessions never worked.</p>
 */
class StickySessionRoundTripTest {

    private Cluster cluster;

    @BeforeEach
    void setup() throws Exception {
        cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(new StickySession()))
                .build();

        // Add 4 backend nodes. The sticky session logic sorts nodes by ID
        // for binary search, so the node returned by the cookie lookup
        // depends on the UUID-based ID ordering, not insertion order.
        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 8080))
                .build();

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.2", 8080))
                .build();

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.3", 8080))
                .build();

        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.4", 8080))
                .build();
    }

    @AfterEach
    void teardown() {
        cluster.close();
    }

    /**
     * Verifies the full sticky session round trip:
     * 1. First request (no cookie) gets a Set-Cookie in the response headers
     * 2. Second request with that cookie is routed to the same node
     */
    @Test
    void firstRequestGetsCookie_subsequentRequestsUseSameNode() throws Exception {
        InetSocketAddress clientAddress = new InetSocketAddress("192.168.1.100", 12345);

        // First request: no cookie -- load balancer selects via round-robin
        HTTPBalanceRequest firstRequest = new HTTPBalanceRequest(clientAddress, EmptyHttpHeaders.INSTANCE);
        HTTPBalanceResponse firstResponse = (HTTPBalanceResponse) cluster.nextNode(firstRequest);

        assertNotNull(firstResponse, "First response must not be null");
        Node firstNode = firstResponse.node();
        assertNotNull(firstNode, "Selected node must not be null");

        // The response must contain a Set-Cookie header with X-SBZ-EGW-RouteID
        HttpHeaders responseHeaders = firstResponse.getHTTPHeaders();
        assertNotNull(responseHeaders, "Response headers must not be null");
        String setCookie = responseHeaders.get(HttpHeaderNames.SET_COOKIE);
        assertNotNull(setCookie, "Set-Cookie header must be present on first request");
        assertTrue(setCookie.contains("X-SBZ-EGW-RouteID"),
                "Set-Cookie must contain the route ID cookie name, got: " + setCookie);

        // Second request: send the cookie back. The load balancer should
        // recognize the cookie and route to the same node.
        DefaultHttpHeaders headersWithCookie = new DefaultHttpHeaders();
        headersWithCookie.add(HttpHeaderNames.COOKIE, setCookie.split(";")[0]); // Extract "name=value" part

        HTTPBalanceRequest secondRequest = new HTTPBalanceRequest(clientAddress, headersWithCookie);
        HTTPBalanceResponse secondResponse = (HTTPBalanceResponse) cluster.nextNode(secondRequest);

        assertNotNull(secondResponse, "Second response must not be null");
        assertSame(firstNode, secondResponse.node(),
                "Second request with cookie must be routed to the same node");
    }

    /**
     * Verifies that multiple clients each get their own sticky session to
     * potentially different nodes, and each client's subsequent requests
     * are routed consistently.
     */
    @Test
    void multipleClients_eachStickyToOwnNode() throws Exception {
        // Simulate 10 different clients
        for (int i = 0; i < 10; i++) {
            InetSocketAddress clientAddress = new InetSocketAddress("192.168.1." + i, 12345);

            // First request (no cookie) -- get assigned a node
            HTTPBalanceRequest firstRequest = new HTTPBalanceRequest(clientAddress, EmptyHttpHeaders.INSTANCE);
            HTTPBalanceResponse firstResponse = (HTTPBalanceResponse) cluster.nextNode(firstRequest);
            Node assignedNode = firstResponse.node();
            assertNotNull(assignedNode);

            // Extract cookie
            String setCookie = firstResponse.getHTTPHeaders().get(HttpHeaderNames.SET_COOKIE);
            assertNotNull(setCookie, "Client " + i + " must receive Set-Cookie");

            // Make 5 subsequent requests with the cookie -- all should go to same node
            for (int j = 0; j < 5; j++) {
                DefaultHttpHeaders headers = new DefaultHttpHeaders();
                headers.add(HttpHeaderNames.COOKIE, setCookie.split(";")[0]);

                HTTPBalanceRequest followup = new HTTPBalanceRequest(clientAddress, headers);
                HTTPBalanceResponse followupResponse = (HTTPBalanceResponse) cluster.nextNode(followup);

                assertSame(assignedNode, followupResponse.node(),
                        "Client " + i + " request " + j + " must route to same node");
            }
        }
    }

    /**
     * Verifies that the Set-Cookie header includes HttpOnly, SameSite, and
     * Path attributes for security.
     */
    @Test
    void cookieAttributes_includeSecurityFlags() throws Exception {
        InetSocketAddress clientAddress = new InetSocketAddress("192.168.1.1", 12345);

        HTTPBalanceRequest request = new HTTPBalanceRequest(clientAddress, EmptyHttpHeaders.INSTANCE);
        HTTPBalanceResponse response = (HTTPBalanceResponse) cluster.nextNode(request);

        String setCookie = response.getHTTPHeaders().get(HttpHeaderNames.SET_COOKIE);
        assertNotNull(setCookie);

        // Verify security attributes per RFC 6265 and StickySession implementation
        String lowerCookie = setCookie.toLowerCase();
        assertTrue(lowerCookie.contains("httponly"), "Cookie must be HttpOnly, got: " + setCookie);
        assertTrue(lowerCookie.contains("samesite=strict"), "Cookie must be SameSite=Strict, got: " + setCookie);
        assertTrue(lowerCookie.contains("path=/"), "Cookie must have Path=/, got: " + setCookie);
    }

    /**
     * Verifies that the Secure flag is set on cookies when StickySession is
     * configured for HTTPS (LB-07 fix).
     */
    @Test
    void secureFlagSetWhenConfigured() throws Exception {
        // Create a new cluster with secure sticky session
        StickySession stickySession = new StickySession().setSecure(true);
        Cluster secureCluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new HTTPRoundRobin(stickySession))
                .build();

        NodeBuilder.newBuilder()
                .withCluster(secureCluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 8443))
                .build();

        try {
            InetSocketAddress clientAddress = new InetSocketAddress("192.168.1.1", 12345);
            HTTPBalanceRequest request = new HTTPBalanceRequest(clientAddress, EmptyHttpHeaders.INSTANCE);
            HTTPBalanceResponse response = (HTTPBalanceResponse) secureCluster.nextNode(request);

            String setCookie = response.getHTTPHeaders().get(HttpHeaderNames.SET_COOKIE);
            assertNotNull(setCookie);
            assertTrue(setCookie.toLowerCase().contains("secure"),
                    "Cookie must include Secure flag when configured, got: " + setCookie);
        } finally {
            secureCluster.close();
        }
    }

    /**
     * Verifies that the cookie value contains the SHA-256 hash of the node's ID
     * (MED-27: cookie no longer leaks raw internal UUIDs).
     */
    @Test
    void cookieValueMatchesNodeId() throws Exception {
        InetSocketAddress clientAddress = new InetSocketAddress("192.168.1.1", 12345);

        HTTPBalanceRequest request = new HTTPBalanceRequest(clientAddress, EmptyHttpHeaders.INSTANCE);
        HTTPBalanceResponse response = (HTTPBalanceResponse) cluster.nextNode(request);

        Node selectedNode = response.node();
        String setCookie = response.getHTTPHeaders().get(HttpHeaderNames.SET_COOKIE);
        assertNotNull(setCookie);

        // MED-27: Cookie value is now SHA-256 hash of the node ID, not the raw UUID
        String hashedId = hashNodeId(selectedNode.id());
        assertTrue(setCookie.contains(hashedId),
                "Cookie value must contain the hashed node ID. " +
                        "Hashed ID: " + hashedId + ", Cookie: " + setCookie);
    }

    private static String hashNodeId(String nodeId) {
        try {
            java.security.MessageDigest digest = java.security.MessageDigest.getInstance("SHA-256");
            byte[] hash = digest.digest(nodeId.getBytes(java.nio.charset.StandardCharsets.UTF_8));
            return java.util.HexFormat.of().formatHex(hash);
        } catch (java.security.NoSuchAlgorithmException e) {
            throw new AssertionError("SHA-256 not available", e);
        }
    }
}
