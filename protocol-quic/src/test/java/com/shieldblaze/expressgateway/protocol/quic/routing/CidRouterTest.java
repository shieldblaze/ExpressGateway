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
package com.shieldblaze.expressgateway.protocol.quic.routing;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.protocol.quic.QuicBackendSession;
import com.shieldblaze.expressgateway.protocol.quic.QuicCidSessionMap;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.InetSocketAddress;
import java.security.SecureRandom;
import java.time.Duration;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link CidRouter} -- CID-based routing with server-ID prefix,
 * CID rotation support (RFC 9000 Section 5.1), and multi-instance routing.
 */
@Timeout(10)
class CidRouterTest {

    private static final SecureRandom SECURE_RANDOM = new SecureRandom();
    private static final byte[] CLUSTER_SECRET = new byte[32];
    static {
        SECURE_RANDOM.nextBytes(CLUSTER_SECRET);
    }

    private Cluster cluster;
    private Node node;
    private QuicCidSessionMap cidSessionMap;
    private CidGenerator cidGenerator;
    private CidRouter cidRouter;

    @BeforeEach
    void setUp() throws Exception {
        cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 4433))
                .build();

        cidSessionMap = new QuicCidSessionMap(Duration.ofSeconds(60));
        cidGenerator = new CidGenerator("server-01".getBytes(), CLUSTER_SECRET, 8);
        cidRouter = new CidRouter(cidGenerator, cidSessionMap);
    }

    @AfterEach
    void tearDown() {
        cidRouter.close();
        cidSessionMap.close();
        cluster.close();
    }

    @Test
    void route_ownCid_withRegisteredSession_returnsSession() {
        QuicBackendSession session = new QuicBackendSession(node);
        byte[] cid = cidGenerator.generate();
        cidRouter.registerCid(cid, session, 8);

        CidRouter.RoutingResult result = cidRouter.route(cid);
        assertTrue(result.ownedByUs());
        assertTrue(result.hasSession());
        assertSame(session, result.session());
    }

    @Test
    void route_ownCid_withoutRegisteredSession_returnsOwnedButNoSession() {
        byte[] cid = cidGenerator.generate();
        // Not registered in session map

        CidRouter.RoutingResult result = cidRouter.route(cid);
        assertTrue(result.ownedByUs());
        assertFalse(result.hasSession());
        assertNull(result.session());
    }

    @Test
    void route_foreignCid_identifiesAsForeign() {
        CidGenerator foreignGen = new CidGenerator("server-02".getBytes(), CLUSTER_SECRET, 8);
        byte[] foreignCid = foreignGen.generate();

        CidRouter.RoutingResult result = cidRouter.route(foreignCid);
        assertFalse(result.ownedByUs());
        assertFalse(result.hasSession());
        assertNotNull(result.targetServerPrefix());
    }

    @Test
    void route_nullOrShortCid_returnsNoMatch() {
        CidRouter.RoutingResult result1 = cidRouter.route(null);
        assertFalse(result1.ownedByUs());
        assertFalse(result1.hasSession());

        CidRouter.RoutingResult result2 = cidRouter.route(new byte[]{0x01});
        assertFalse(result2.ownedByUs());
        assertFalse(result2.hasSession());
    }

    @Test
    void issueNewCid_registersInSessionMap_andHasSamePrefix() {
        QuicBackendSession session = new QuicBackendSession(node);

        byte[] newCid = cidRouter.issueNewCid(session);
        assertNotNull(newCid);
        assertEquals(8, newCid.length);

        // New CID should be routable to the same session
        CidRouter.RoutingResult result = cidRouter.route(newCid);
        assertTrue(result.ownedByUs());
        assertTrue(result.hasSession());
        assertSame(session, result.session());
    }

    @Test
    void cidRotation_multipleNewCids_allRouteToSameSession() {
        QuicBackendSession session = new QuicBackendSession(node);

        byte[] cid1 = cidRouter.issueNewCid(session);
        byte[] cid2 = cidRouter.issueNewCid(session);
        byte[] cid3 = cidRouter.issueNewCid(session);

        assertSame(session, cidRouter.route(cid1).session());
        assertSame(session, cidRouter.route(cid2).session());
        assertSame(session, cidRouter.route(cid3).session());
    }

    @Test
    void retireCid_removesMapping_otherCidsUnaffected() {
        QuicBackendSession session = new QuicBackendSession(node);
        byte[] cid1 = cidRouter.issueNewCid(session);
        byte[] cid2 = cidRouter.issueNewCid(session);

        cidRouter.retireCid(cid1);

        assertNull(cidRouter.route(cid1).session(), "Retired CID must not resolve");
        assertSame(session, cidRouter.route(cid2).session(), "Non-retired CID must still resolve");
    }

    @Test
    void serverRegistry_registerAndResolve() {
        byte[] foreignPrefix = CidGenerator.computeServerIdPrefix("server-02".getBytes(), CLUSTER_SECRET);
        cidRouter.registerServer(foreignPrefix, "lb-02:10.0.1.2:4433");

        String resolved = cidRouter.resolveServer(foreignPrefix);
        assertEquals("lb-02:10.0.1.2:4433", resolved);
    }

    @Test
    void serverRegistry_deregister() {
        byte[] foreignPrefix = CidGenerator.computeServerIdPrefix("server-02".getBytes(), CLUSTER_SECRET);
        cidRouter.registerServer(foreignPrefix, "lb-02:10.0.1.2:4433");
        cidRouter.deregisterServer(foreignPrefix);

        assertNull(cidRouter.resolveServer(foreignPrefix));
    }

    @Test
    void metrics_trackLookups() {
        QuicBackendSession session = new QuicBackendSession(node);
        byte[] ownCid = cidGenerator.generate();
        cidRouter.registerCid(ownCid, session, 8);

        CidGenerator foreignGen = new CidGenerator("server-02".getBytes(), CLUSTER_SECRET, 8);
        byte[] foreignCid = foreignGen.generate();

        cidRouter.route(ownCid);      // local hit
        cidRouter.route(foreignCid);   // foreign hit
        cidRouter.route(ownCid);      // local hit

        assertEquals(3, cidRouter.totalLookups());
        assertEquals(2, cidRouter.localHits());
        assertEquals(1, cidRouter.foreignHits());
    }

    // ═══════════════════════════════════════════════════════════════════════
    // CID rotation tracking with primary CID association
    // ═══════════════════════════════════════════════════════════════════════

    @Test
    void cidRotation_withPrimaryCid_allMapToSamePrimary() {
        QuicBackendSession session = new QuicBackendSession(node);
        byte[] primaryCid = cidGenerator.generate();
        cidRouter.registerCid(primaryCid, session, 8);

        // Issue rotated CIDs associated with the primary
        byte[] cid2 = cidRouter.issueNewCid(session, primaryCid);
        byte[] cid3 = cidRouter.issueNewCid(session, primaryCid);

        // All CIDs should map to the same primary
        assertEquals(cidRouter.getPrimaryCid(primaryCid), cidRouter.getPrimaryCid(primaryCid));
        assertEquals(cidRouter.getPrimaryCid(primaryCid), cidRouter.getPrimaryCid(cid2));
        assertEquals(cidRouter.getPrimaryCid(primaryCid), cidRouter.getPrimaryCid(cid3));

        // All CIDs should route to the same session
        assertSame(session, cidRouter.route(primaryCid).session());
        assertSame(session, cidRouter.route(cid2).session());
        assertSame(session, cidRouter.route(cid3).session());
    }

    @Test
    void retireConnection_removesAllCids() {
        QuicBackendSession session = new QuicBackendSession(node);
        byte[] primaryCid = cidGenerator.generate();
        cidRouter.registerCid(primaryCid, session, 8);

        byte[] cid2 = cidRouter.issueNewCid(session, primaryCid);
        byte[] cid3 = cidRouter.issueNewCid(session, primaryCid);

        // All three should be tracked
        assertNotNull(cidRouter.getConnectionCids(primaryCid));
        assertEquals(3, cidRouter.getConnectionCids(primaryCid).size());

        // Retire the entire connection
        cidRouter.retireConnection(primaryCid);

        // All CIDs should be gone
        assertNull(cidRouter.route(primaryCid).session());
        assertNull(cidRouter.route(cid2).session());
        assertNull(cidRouter.route(cid3).session());
        assertNull(cidRouter.getConnectionCids(primaryCid));
        assertNull(cidRouter.getPrimaryCid(primaryCid));
        assertNull(cidRouter.getPrimaryCid(cid2));
        assertNull(cidRouter.getPrimaryCid(cid3));
    }

    @Test
    void retireCid_removesFromBothMaps_leavesOtherCids() {
        QuicBackendSession session = new QuicBackendSession(node);
        byte[] primaryCid = cidGenerator.generate();
        cidRouter.registerCid(primaryCid, session, 8);

        byte[] cid2 = cidRouter.issueNewCid(session, primaryCid);
        byte[] cid3 = cidRouter.issueNewCid(session, primaryCid);

        // Retire cid2 only
        cidRouter.retireCid(cid2);

        // cid2 should be gone from both maps
        assertNull(cidRouter.route(cid2).session());
        assertNull(cidRouter.getPrimaryCid(cid2));

        // Primary and cid3 should still work
        assertSame(session, cidRouter.route(primaryCid).session());
        assertSame(session, cidRouter.route(cid3).session());
        assertNotNull(cidRouter.getConnectionCids(primaryCid));
        assertEquals(2, cidRouter.getConnectionCids(primaryCid).size());
    }

    @Test
    void registerCid_withExplicitPrimaryCid() {
        QuicBackendSession session = new QuicBackendSession(node);
        byte[] primaryCid = cidGenerator.generate();
        cidRouter.registerCid(primaryCid, session, 8);

        byte[] secondaryCid = cidGenerator.generate();
        cidRouter.registerCid(secondaryCid, session, 8, primaryCid);

        // Both should map to the same primary
        assertEquals(cidRouter.getPrimaryCid(primaryCid), cidRouter.getPrimaryCid(secondaryCid));

        // Both should route to the same session
        assertSame(session, cidRouter.route(primaryCid).session());
        assertSame(session, cidRouter.route(secondaryCid).session());
    }
}
