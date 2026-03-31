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
package com.shieldblaze.expressgateway.protocol.quic;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.InetSocketAddress;
import java.time.Duration;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * RB-TEST-01: Tests for {@link QuicCidSessionMap} -- CID-based session routing
 * for QUIC connection migration support per RFC 9000 Section 9.
 *
 * <p>These tests verify that:
 * <ul>
 *   <li>CID-to-session mapping works correctly (put/get/remove)</li>
 *   <li>The same CID always routes to the same backend session (connection migration)</li>
 *   <li>Different CIDs from the same client can map to the same session</li>
 *   <li>CID length tracking works for Short Header parsing</li>
 *   <li>Idle eviction removes expired entries</li>
 * </ul>
 */
@Timeout(value = 30)
class QuicCidSessionMapTest {

    private Cluster cluster;
    private Node node1;
    private Node node2;
    private QuicCidSessionMap cidMap;

    @BeforeEach
    void setUp() throws Exception {
        cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        node1 = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 4433))
                .build();

        node2 = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.2", 4433))
                .build();

        cidMap = new QuicCidSessionMap(Duration.ofSeconds(60));
    }

    @AfterEach
    void tearDown() {
        if (cidMap != null) cidMap.close();
        if (cluster != null) cluster.close();
    }

    @Test
    void putAndGet_returnsCorrectSession() {
        QuicBackendSession session = new QuicBackendSession(node1);
        byte[] cid = {0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08};

        cidMap.put(cid, session, 8);

        QuicBackendSession result = cidMap.get(cid);
        assertNotNull(result, "get() must return the mapped session");
        assertSame(session, result, "get() must return the exact same session instance");
        assertEquals(1, cidMap.size());
    }

    @Test
    void get_unknownCid_returnsNull() {
        byte[] unknownCid = {(byte) 0xFF, (byte) 0xFE, (byte) 0xFD};
        assertNull(cidMap.get(unknownCid), "get() for unknown CID must return null");
    }

    @Test
    void sameCid_alwaysRoutesToSameSession_connectionMigration() {
        // This simulates QUIC connection migration: the client's source address changes
        // (e.g., WiFi -> cellular), but the CID stays the same. The proxy must route
        // the migrated connection to the same backend session.
        QuicBackendSession session = new QuicBackendSession(node1);
        byte[] cid = {0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11};

        cidMap.put(cid, session, 8);

        // First lookup -- "from WiFi address"
        QuicBackendSession fromWifi = cidMap.get(cid);
        assertSame(session, fromWifi);

        // Second lookup -- "from cellular address" (same CID, different source IP)
        // The CID map is IP-agnostic -- it only cares about the CID bytes.
        QuicBackendSession fromCellular = cidMap.get(cid);
        assertSame(session, fromCellular, "Same CID must always route to same session regardless of source IP");
        assertSame(fromWifi, fromCellular);
    }

    @Test
    void multipleCids_sameSession_allResolve() {
        // A single QUIC connection can have multiple CIDs (RFC 9000 Section 5.1.1).
        // All CIDs for the same session must route to the same backend.
        QuicBackendSession session = new QuicBackendSession(node1);
        byte[] cid1 = {0x01, 0x02, 0x03, 0x04};
        byte[] cid2 = {0x05, 0x06, 0x07, 0x08};
        byte[] cid3 = {0x09, 0x0A, 0x0B, 0x0C};

        cidMap.put(cid1, session, 4);
        cidMap.put(cid2, session, 4);
        cidMap.put(cid3, session, 4);

        assertSame(session, cidMap.get(cid1));
        assertSame(session, cidMap.get(cid2));
        assertSame(session, cidMap.get(cid3));
        assertEquals(3, cidMap.size());
    }

    @Test
    void differentCids_differentSessions() {
        QuicBackendSession session1 = new QuicBackendSession(node1);
        QuicBackendSession session2 = new QuicBackendSession(node2);
        byte[] cid1 = {0x01, 0x02, 0x03, 0x04};
        byte[] cid2 = {0x05, 0x06, 0x07, 0x08};

        cidMap.put(cid1, session1, 4);
        cidMap.put(cid2, session2, 4);

        assertSame(session1, cidMap.get(cid1));
        assertSame(session2, cidMap.get(cid2));
    }

    @Test
    void remove_clearsMapping() {
        QuicBackendSession session = new QuicBackendSession(node1);
        byte[] cid = {0x01, 0x02, 0x03, 0x04};

        cidMap.put(cid, session, 4);
        assertEquals(1, cidMap.size());

        cidMap.remove(cid);
        assertNull(cidMap.get(cid), "Removed CID must return null");
        assertEquals(0, cidMap.size());
    }

    @Test
    void knownCidLengths_tracksDistinctLengths() {
        QuicBackendSession session1 = new QuicBackendSession(node1);
        QuicBackendSession session2 = new QuicBackendSession(node2);

        // Add entries with different CID lengths
        cidMap.put(new byte[]{0x01, 0x02, 0x03, 0x04}, session1, 4);
        cidMap.put(new byte[]{0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C}, session2, 8);

        int[] lengths = cidMap.knownCidLengths();
        assertEquals(2, lengths.length, "Must track two distinct CID lengths");

        // Check that both lengths are present (order may vary)
        boolean has4 = false, has8 = false;
        for (int len : lengths) {
            if (len == 4) has4 = true;
            if (len == 8) has8 = true;
        }
        assertTrue(has4, "Must contain CID length 4");
        assertTrue(has8, "Must contain CID length 8");
    }

    @Test
    void knownCidLengths_decrementedOnRemove() {
        QuicBackendSession session = new QuicBackendSession(node1);
        byte[] cid = {0x01, 0x02, 0x03, 0x04};

        cidMap.put(cid, session, 4);
        assertEquals(1, cidMap.knownCidLengths().length);

        cidMap.remove(cid);
        assertEquals(0, cidMap.knownCidLengths().length,
                "CID length must be removed when last entry of that length is removed");
    }

    @Test
    void idleEviction_removesExpiredEntries() throws Exception {
        // Use a very short idle timeout for testing
        cidMap.close();
        cidMap = new QuicCidSessionMap(Duration.ofSeconds(1));

        QuicBackendSession session = new QuicBackendSession(node1);
        byte[] cid = {0x01, 0x02, 0x03, 0x04};

        cidMap.put(cid, session, 4);
        assertEquals(1, cidMap.size());

        // Wait for the idle timeout + eviction sweep interval
        Thread.sleep(3000);

        assertNull(cidMap.get(cid), "Expired CID must be evicted after idle timeout");
        assertEquals(0, cidMap.size());
    }

    @Test
    void get_refreshesAccessTimestamp_preventsEviction() throws Exception {
        cidMap.close();
        cidMap = new QuicCidSessionMap(Duration.ofSeconds(2));

        QuicBackendSession session = new QuicBackendSession(node1);
        byte[] cid = {0x01, 0x02, 0x03, 0x04};

        cidMap.put(cid, session, 4);

        // Access the entry periodically to keep it alive
        for (int i = 0; i < 4; i++) {
            Thread.sleep(800);
            assertNotNull(cidMap.get(cid), "Active CID must not be evicted (access #" + i + ")");
        }

        // Entry should still be alive after periodic access
        assertNotNull(cidMap.get(cid), "CID with refreshed access must survive eviction");
    }

    @Test
    void close_clearsAllState() {
        QuicBackendSession session = new QuicBackendSession(node1);
        cidMap.put(new byte[]{0x01}, session, 1);
        cidMap.put(new byte[]{0x02, 0x03}, session, 2);

        assertEquals(2, cidMap.size());

        cidMap.close();
        assertEquals(0, cidMap.size());
        assertEquals(0, cidMap.knownCidLengths().length);
    }
}
