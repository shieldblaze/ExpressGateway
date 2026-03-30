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
package com.shieldblaze.expressgateway.protocol.quic.nat;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.protocol.quic.QuicBackendSession;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.InetSocketAddress;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link NatRebindingDetector} -- detecting and handling NAT rebinding
 * per RFC 9000 Section 9.3.
 */
@Timeout(10)
class NatRebindingDetectorTest {

    private Cluster cluster;
    private Node node;

    @BeforeEach
    void setUp() throws Exception {
        cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(NOOPSessionPersistence.INSTANCE))
                .build();

        node = NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress("10.0.0.1", 4433))
                .build();
    }

    @AfterEach
    void tearDown() {
        cluster.close();
    }

    @Test
    void isNatRebinding_sameIpDifferentPort_true() {
        NatRebindingDetector detector = new NatRebindingDetector();
        InetSocketAddress old = new InetSocketAddress("192.168.1.100", 12345);
        InetSocketAddress rebound = new InetSocketAddress("192.168.1.100", 54321);
        assertTrue(detector.isNatRebinding(old, rebound));
    }

    @Test
    void isNatRebinding_differentIp_false() {
        NatRebindingDetector detector = new NatRebindingDetector();
        InetSocketAddress old = new InetSocketAddress("192.168.1.100", 12345);
        InetSocketAddress migrated = new InetSocketAddress("10.0.0.50", 12345);
        assertFalse(detector.isNatRebinding(old, migrated));
    }

    @Test
    void isNatRebinding_sameIpSamePort_false() {
        NatRebindingDetector detector = new NatRebindingDetector();
        InetSocketAddress addr = new InetSocketAddress("192.168.1.100", 12345);
        assertFalse(detector.isNatRebinding(addr, addr));
    }

    @Test
    void isNatRebinding_nullInputs_false() {
        NatRebindingDetector detector = new NatRebindingDetector();
        assertFalse(detector.isNatRebinding(null, new InetSocketAddress("1.2.3.4", 1234)));
        assertFalse(detector.isNatRebinding(new InetSocketAddress("1.2.3.4", 1234), null));
        assertFalse(detector.isNatRebinding(null, null));
    }

    @Test
    void handleRebinding_updatesAddressMap() {
        NatRebindingDetector detector = new NatRebindingDetector();
        QuicBackendSession session = new QuicBackendSession(node);
        Map<InetSocketAddress, QuicBackendSession> addressMap = new ConcurrentHashMap<>();

        InetSocketAddress oldAddr = new InetSocketAddress("192.168.1.100", 12345);
        InetSocketAddress newAddr = new InetSocketAddress("192.168.1.100", 54321);

        boolean accepted = detector.handleRebinding(oldAddr, newAddr, session, addressMap);
        assertTrue(accepted);
        assertSame(session, addressMap.get(newAddr));
        assertEquals(1, detector.totalRebindings());
    }

    @Test
    void handleRebinding_rateLimited_afterMaxPerWindow() {
        // Max 3 rebindings per 10-second window
        NatRebindingDetector detector = new NatRebindingDetector(3, 10_000_000_000L);
        QuicBackendSession session = new QuicBackendSession(node);
        Map<InetSocketAddress, QuicBackendSession> addressMap = new ConcurrentHashMap<>();

        InetSocketAddress oldAddr = new InetSocketAddress("192.168.1.100", 12345);

        // First 3 rebindings: accepted
        for (int i = 0; i < 3; i++) {
            InetSocketAddress newAddr = new InetSocketAddress("192.168.1.100", 20000 + i);
            assertTrue(detector.handleRebinding(oldAddr, newAddr, session, addressMap),
                    "Rebinding #" + (i + 1) + " must be accepted");
        }

        // 4th rebinding: rate-limited
        InetSocketAddress newAddr4 = new InetSocketAddress("192.168.1.100", 30000);
        assertFalse(detector.handleRebinding(oldAddr, newAddr4, session, addressMap),
                "Rebinding beyond rate limit must be rejected");
        assertEquals(1, detector.rejectedRebindings());
    }

    @Test
    void isRateLimited_respectsWindow() {
        // Use a 10-second window so the window does not expire during the test
        NatRebindingDetector detector = new NatRebindingDetector(2, 10_000_000_000L);
        QuicBackendSession session = new QuicBackendSession(node);
        Map<InetSocketAddress, QuicBackendSession> addressMap = new ConcurrentHashMap<>();

        InetSocketAddress oldAddr = new InetSocketAddress("192.168.1.100", 12345);

        // Exhaust the limit
        for (int i = 0; i < 2; i++) {
            InetSocketAddress newAddr = new InetSocketAddress("192.168.1.100", 20000 + i);
            detector.handleRebinding(oldAddr, newAddr, session, addressMap);
        }

        InetSocketAddress checkAddr = new InetSocketAddress("192.168.1.100", 30000);
        assertTrue(detector.isRateLimited(checkAddr));
    }

    @Test
    void evictExpired_removesOldCounters() throws Exception {
        NatRebindingDetector detector = new NatRebindingDetector(5, 1_000_000L); // 1ms window
        QuicBackendSession session = new QuicBackendSession(node);
        Map<InetSocketAddress, QuicBackendSession> addressMap = new ConcurrentHashMap<>();

        InetSocketAddress oldAddr = new InetSocketAddress("192.168.1.100", 12345);
        InetSocketAddress newAddr = new InetSocketAddress("192.168.1.100", 54321);
        detector.handleRebinding(oldAddr, newAddr, session, addressMap);

        Thread.sleep(50); // Wait beyond window * 6 (eviction threshold)
        int evicted = detector.evictExpired();
        assertTrue(evicted > 0, "Expired counters must be evicted");
    }

    @Test
    void differentIpAddresses_independentRateLimits() {
        NatRebindingDetector detector = new NatRebindingDetector(1, 10_000_000_000L);
        QuicBackendSession session = new QuicBackendSession(node);
        Map<InetSocketAddress, QuicBackendSession> addressMap = new ConcurrentHashMap<>();

        // First IP: use up the limit
        InetSocketAddress old1 = new InetSocketAddress("192.168.1.100", 12345);
        InetSocketAddress new1 = new InetSocketAddress("192.168.1.100", 54321);
        assertTrue(detector.handleRebinding(old1, new1, session, addressMap));

        // Second IP: independent limit, should be accepted
        InetSocketAddress old2 = new InetSocketAddress("10.0.0.50", 12345);
        InetSocketAddress new2 = new InetSocketAddress("10.0.0.50", 54321);
        assertTrue(detector.handleRebinding(old2, new2, session, addressMap),
                "Different IPs must have independent rate limits");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Old address cleanup
    // ═══════════════════════════════════════════════════════════════════════

    @Test
    void handleRebinding_removesOldAddressFromMap() {
        NatRebindingDetector detector = new NatRebindingDetector();
        QuicBackendSession session = new QuicBackendSession(node);
        Map<InetSocketAddress, QuicBackendSession> addressMap = new ConcurrentHashMap<>();

        InetSocketAddress oldAddr = new InetSocketAddress("192.168.1.100", 12345);
        InetSocketAddress newAddr = new InetSocketAddress("192.168.1.100", 54321);

        // Pre-populate old address mapping
        addressMap.put(oldAddr, session);

        boolean accepted = detector.handleRebinding(oldAddr, newAddr, session, addressMap);
        assertTrue(accepted);

        // Old address must be removed
        assertNull(addressMap.get(oldAddr),
                "Old address mapping must be removed to prevent stale entries");
        // New address must be present
        assertSame(session, addressMap.get(newAddr));
    }

    @Test
    void handleRebinding_multipleRebindings_cleanupChain() {
        NatRebindingDetector detector = new NatRebindingDetector(10, 10_000_000_000L);
        QuicBackendSession session = new QuicBackendSession(node);
        Map<InetSocketAddress, QuicBackendSession> addressMap = new ConcurrentHashMap<>();

        InetSocketAddress addr1 = new InetSocketAddress("192.168.1.100", 10001);
        InetSocketAddress addr2 = new InetSocketAddress("192.168.1.100", 10002);
        InetSocketAddress addr3 = new InetSocketAddress("192.168.1.100", 10003);

        addressMap.put(addr1, session);

        // First rebinding: addr1 -> addr2
        detector.handleRebinding(addr1, addr2, session, addressMap);
        assertNull(addressMap.get(addr1));
        assertSame(session, addressMap.get(addr2));

        // Second rebinding: addr2 -> addr3
        detector.handleRebinding(addr2, addr3, session, addressMap);
        assertNull(addressMap.get(addr2));
        assertSame(session, addressMap.get(addr3));

        // Only the latest address should remain
        assertEquals(1, addressMap.size());
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Rate limiter atomic correctness
    // ═══════════════════════════════════════════════════════════════════════

    @Test
    void rateLimiter_atomicInsideCompute() {
        // Max 1 rebinding per window. The rate limiter check and increment must be
        // atomic inside compute() to prevent TOCTOU where two concurrent calls
        // both pass the check before either increments.
        NatRebindingDetector detector = new NatRebindingDetector(1, 10_000_000_000L);
        QuicBackendSession session = new QuicBackendSession(node);
        Map<InetSocketAddress, QuicBackendSession> addressMap = new ConcurrentHashMap<>();

        InetSocketAddress oldAddr = new InetSocketAddress("192.168.1.100", 12345);
        InetSocketAddress newAddr1 = new InetSocketAddress("192.168.1.100", 20001);
        InetSocketAddress newAddr2 = new InetSocketAddress("192.168.1.100", 20002);

        // First rebinding: accepted
        assertTrue(detector.handleRebinding(oldAddr, newAddr1, session, addressMap));

        // Second rebinding: must be rejected (at limit)
        assertFalse(detector.handleRebinding(newAddr1, newAddr2, session, addressMap),
                "Rate limiter must atomically check and reject when at limit");
    }
}
