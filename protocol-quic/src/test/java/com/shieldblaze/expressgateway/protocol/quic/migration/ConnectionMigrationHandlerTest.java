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
package com.shieldblaze.expressgateway.protocol.quic.migration;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.protocol.quic.QuicBackendSession;
import com.shieldblaze.expressgateway.protocol.quic.QuicCidSessionMap;
import com.shieldblaze.expressgateway.protocol.quic.nat.NatRebindingDetector;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.net.InetSocketAddress;
import java.time.Duration;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link ConnectionMigrationHandler} -- QUIC connection migration
 * with PATH_CHALLENGE/PATH_RESPONSE validation, anti-amplification, rate limiting,
 * and NAT rebinding detection per RFC 9000 Section 9.
 */
@Timeout(10)
class ConnectionMigrationHandlerTest {

    private Cluster cluster;
    private Node node;
    private QuicCidSessionMap cidSessionMap;
    private PathValidator pathValidator;
    private NatRebindingDetector natDetector;
    private Map<InetSocketAddress, QuicBackendSession> addressSessionMap;
    private ConnectionMigrationHandler migrationHandler;

    private static final InetSocketAddress WIFI_ADDR = new InetSocketAddress("192.168.1.100", 12345);
    private static final InetSocketAddress CELL_ADDR = new InetSocketAddress("10.0.0.50", 54321);
    private static final InetSocketAddress NAT_REBOUND_ADDR = new InetSocketAddress("192.168.1.100", 23456);
    private static final byte[] TEST_DCID = {0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08};

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
        pathValidator = new PathValidator();
        natDetector = new NatRebindingDetector();
        addressSessionMap = new ConcurrentHashMap<>();
        migrationHandler = new ConnectionMigrationHandler(
                pathValidator, natDetector, addressSessionMap, cidSessionMap);
    }

    @AfterEach
    void tearDown() {
        cidSessionMap.close();
        cluster.close();
    }

    @Test
    void fullMigration_initiatesPathChallenge() {
        QuicBackendSession session = new QuicBackendSession(node);
        cidSessionMap.put(TEST_DCID, session, 8);
        addressSessionMap.put(WIFI_ADDR, session);

        byte[] challenge = migrationHandler.handleMigration(
                WIFI_ADDR, CELL_ADDR, TEST_DCID, session, 1200);

        assertNotNull(challenge, "Full migration must initiate PATH_CHALLENGE");
        assertEquals(8, challenge.length, "PATH_CHALLENGE data must be 8 bytes");
        assertTrue(migrationHandler.isMigrating(CELL_ADDR));
        assertEquals(1, migrationHandler.totalMigrations());
    }

    @Test
    void fullMigration_pathResponseCompletesMigration() {
        QuicBackendSession session = new QuicBackendSession(node);
        cidSessionMap.put(TEST_DCID, session, 8);
        addressSessionMap.put(WIFI_ADDR, session);

        byte[] challenge = migrationHandler.handleMigration(
                WIFI_ADDR, CELL_ADDR, TEST_DCID, session, 1200);

        // Simulate PATH_RESPONSE with the correct challenge data
        ConnectionMigrationHandler.MigrationState result =
                migrationHandler.processPathResponse(CELL_ADDR, challenge);

        assertEquals(ConnectionMigrationHandler.MigrationState.COMPLETED, result);
        assertFalse(migrationHandler.isMigrating(CELL_ADDR));
        assertEquals(1, migrationHandler.successfulMigrations());

        // Address map should be updated
        assertSame(session, addressSessionMap.get(CELL_ADDR));
    }

    @Test
    void fullMigration_wrongPathResponse_fails() {
        QuicBackendSession session = new QuicBackendSession(node);
        cidSessionMap.put(TEST_DCID, session, 8);

        migrationHandler.handleMigration(WIFI_ADDR, CELL_ADDR, TEST_DCID, session, 1200);

        byte[] wrongData = {0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00};
        ConnectionMigrationHandler.MigrationState result =
                migrationHandler.processPathResponse(CELL_ADDR, wrongData);

        assertEquals(ConnectionMigrationHandler.MigrationState.FAILED, result);
    }

    @Test
    void natRebinding_portOnlyChange_handledWithoutPathChallenge() {
        QuicBackendSession session = new QuicBackendSession(node);
        addressSessionMap.put(WIFI_ADDR, session);

        byte[] challenge = migrationHandler.handleMigration(
                WIFI_ADDR, NAT_REBOUND_ADDR, TEST_DCID, session, 1200);

        assertNull(challenge, "NAT rebinding must NOT require PATH_CHALLENGE");
        assertFalse(migrationHandler.isMigrating(NAT_REBOUND_ADDR));
        assertEquals(1, migrationHandler.natRebindings());

        // Address mapping should be updated for the new port
        assertSame(session, addressSessionMap.get(NAT_REBOUND_ADDR));
    }

    @Test
    void antiAmplification_limitsResponseBytes() {
        QuicBackendSession session = new QuicBackendSession(node);
        cidSessionMap.put(TEST_DCID, session, 8);

        migrationHandler.handleMigration(WIFI_ADDR, CELL_ADDR, TEST_DCID, session, 100);

        // Anti-amplification: can send up to 3x received (3 * 100 = 300)
        assertTrue(migrationHandler.canSendTo(CELL_ADDR, 300));
        assertFalse(migrationHandler.canSendTo(CELL_ADDR, 301));
    }

    @Test
    void antiAmplification_tracksReceivedAndSentBytes() {
        QuicBackendSession session = new QuicBackendSession(node);
        cidSessionMap.put(TEST_DCID, session, 8);

        migrationHandler.handleMigration(WIFI_ADDR, CELL_ADDR, TEST_DCID, session, 100);

        // Receive more bytes from new address
        migrationHandler.trackBytesReceived(CELL_ADDR, 200);
        // Now received = 300, limit = 900
        assertTrue(migrationHandler.canSendTo(CELL_ADDR, 900));
        assertFalse(migrationHandler.canSendTo(CELL_ADDR, 901));

        // Track sent bytes
        migrationHandler.trackBytesSent(CELL_ADDR, 500);
        // Sent = 500, limit = 900 - 500 = 400 remaining
        assertTrue(migrationHandler.canSendTo(CELL_ADDR, 400));
        assertFalse(migrationHandler.canSendTo(CELL_ADDR, 401));
    }

    @Test
    void rateLimiting_rejectsRapidMigrations() {
        // Use a 1-second rate limit
        migrationHandler = new ConnectionMigrationHandler(
                pathValidator, natDetector, addressSessionMap, cidSessionMap, 1_000_000_000L);

        QuicBackendSession session = new QuicBackendSession(node);
        cidSessionMap.put(TEST_DCID, session, 8);

        InetSocketAddress newAddr1 = new InetSocketAddress("10.0.0.50", 54321);
        InetSocketAddress newAddr2 = new InetSocketAddress("10.0.0.51", 54322);

        // First migration: allowed
        byte[] challenge1 = migrationHandler.handleMigration(
                WIFI_ADDR, newAddr1, TEST_DCID, session, 1200);
        assertNotNull(challenge1);

        // Second migration from same old address: should be rate-limited
        byte[] challenge2 = migrationHandler.handleMigration(
                WIFI_ADDR, newAddr2, TEST_DCID, session, 1200);
        assertNull(challenge2, "Rapid migration from same address must be rate-limited");
        assertEquals(1, migrationHandler.rejectedMigrations());
    }

    @Test
    void noActiveMigration_canSendUnlimited() {
        InetSocketAddress randomAddr = new InetSocketAddress("1.2.3.4", 9999);
        assertTrue(migrationHandler.canSendTo(randomAddr, Long.MAX_VALUE),
                "Without active migration, no amplification limit applies");
    }

    @Test
    void evictStaleMigrations_removesOldEvents() throws Exception {
        QuicBackendSession session = new QuicBackendSession(node);
        cidSessionMap.put(TEST_DCID, session, 8);

        migrationHandler.handleMigration(WIFI_ADDR, CELL_ADDR, TEST_DCID, session, 1200);
        assertTrue(migrationHandler.isMigrating(CELL_ADDR));

        // Evict with a very short max age
        Thread.sleep(50);
        int evicted = migrationHandler.evictStaleMigrations(1_000_000L); // 1ms max age
        assertEquals(1, evicted);
        assertFalse(migrationHandler.isMigrating(CELL_ADDR));
    }

    @Test
    void connectionContinuity_sessionMaintainedAcrossMigration() {
        // This test verifies the critical property: connection continuity across migration.
        // The same backend session must be accessible via CID after a successful migration.
        QuicBackendSession session = new QuicBackendSession(node);
        cidSessionMap.put(TEST_DCID, session, 8);
        addressSessionMap.put(WIFI_ADDR, session);

        // Migrate from WiFi to cellular
        byte[] challenge = migrationHandler.handleMigration(
                WIFI_ADDR, CELL_ADDR, TEST_DCID, session, 1200);
        migrationHandler.processPathResponse(CELL_ADDR, challenge);

        // Session must still be accessible via CID (connection continuity)
        QuicBackendSession fromCid = cidSessionMap.get(TEST_DCID);
        assertSame(session, fromCid, "CRITICAL: Session must be preserved across migration via CID");

        // Session must also be accessible via new address
        QuicBackendSession fromNewAddr = addressSessionMap.get(CELL_ADDR);
        assertSame(session, fromNewAddr, "Session must be accessible via new address after migration");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Atomic amplification budget (fix for TOCTOU race)
    // ═══════════════════════════════════════════════════════════════════════

    @Test
    void tryConsumeAmplificationBudget_atomicallyChecksAndUpdates() {
        QuicBackendSession session = new QuicBackendSession(node);
        cidSessionMap.put(TEST_DCID, session, 8);

        migrationHandler.handleMigration(WIFI_ADDR, CELL_ADDR, TEST_DCID, session, 100);

        // Budget = 3 * 100 = 300
        assertTrue(migrationHandler.tryConsumeAmplificationBudget(CELL_ADDR, 200));
        // Remaining = 300 - 200 = 100
        assertTrue(migrationHandler.tryConsumeAmplificationBudget(CELL_ADDR, 100));
        // Remaining = 0
        assertFalse(migrationHandler.tryConsumeAmplificationBudget(CELL_ADDR, 1),
                "Budget exhausted; must reject");
    }

    @Test
    void tryConsumeAmplificationBudget_noActiveMigration_returnsTrue() {
        InetSocketAddress randomAddr = new InetSocketAddress("1.2.3.4", 9999);
        assertTrue(migrationHandler.tryConsumeAmplificationBudget(randomAddr, Long.MAX_VALUE),
                "Without active migration, no amplification limit applies");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Old-path PATH_CHALLENGE (RFC 9000 Section 9.3.2)
    // ═══════════════════════════════════════════════════════════════════════

    @Test
    void fullMigration_initiatesPathChallengeOnOldPath() {
        QuicBackendSession session = new QuicBackendSession(node);
        cidSessionMap.put(TEST_DCID, session, 8);
        addressSessionMap.put(WIFI_ADDR, session);

        migrationHandler.handleMigration(WIFI_ADDR, CELL_ADDR, TEST_DCID, session, 1200);

        // Verify PATH_CHALLENGE was issued for the OLD address (RFC 9000 Section 9.3.2)
        assertTrue(migrationHandler.pathValidator().hasPendingChallenge(WIFI_ADDR),
                "A PATH_CHALLENGE must be sent to the old address per RFC 9000 Section 9.3.2");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Composite migration key (dcid + newAddress)
    // ═══════════════════════════════════════════════════════════════════════

    @Test
    void compositeMigrationKey_differentDcidsSameAddress() {
        QuicBackendSession session1 = new QuicBackendSession(node);
        QuicBackendSession session2 = new QuicBackendSession(node);
        byte[] dcid1 = {0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08};
        byte[] dcid2 = {0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11};
        cidSessionMap.put(dcid1, session1, 8);
        cidSessionMap.put(dcid2, session2, 8);

        InetSocketAddress oldAddr1 = new InetSocketAddress("192.168.1.100", 12345);
        InetSocketAddress oldAddr2 = new InetSocketAddress("192.168.1.101", 12346);
        InetSocketAddress sameNewAddr = new InetSocketAddress("10.0.0.50", 54321);

        // Two different connections migrating to the same new address
        byte[] challenge1 = migrationHandler.handleMigration(
                oldAddr1, sameNewAddr, dcid1, session1, 1200);
        assertNotNull(challenge1);

        // Second migration from different old address (different connection)
        // This should not be rate-limited because it's from a different source
        byte[] challenge2 = migrationHandler.handleMigration(
                oldAddr2, sameNewAddr, dcid2, session2, 1200);
        assertNotNull(challenge2, "Different connections to same new address must not collide");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Rate limiter atomic correctness
    // ═══════════════════════════════════════════════════════════════════════

    @Test
    void rateLimiter_atomicCheckAndUpdate() {
        // Use a very long rate limit to ensure it doesn't expire during the test
        migrationHandler = new ConnectionMigrationHandler(
                pathValidator, natDetector, addressSessionMap, cidSessionMap, 60_000_000_000L);

        QuicBackendSession session = new QuicBackendSession(node);
        cidSessionMap.put(TEST_DCID, session, 8);

        InetSocketAddress newAddr1 = new InetSocketAddress("10.0.0.50", 54321);
        InetSocketAddress newAddr2 = new InetSocketAddress("10.0.0.51", 54322);

        // First migration from WIFI_ADDR: allowed
        byte[] c1 = migrationHandler.handleMigration(WIFI_ADDR, newAddr1, TEST_DCID, session, 1200);
        assertNotNull(c1);

        // Second from same source: rate-limited
        byte[] c2 = migrationHandler.handleMigration(WIFI_ADDR, newAddr2, TEST_DCID, session, 1200);
        assertNull(c2, "Rate limiter must atomically reject rapid migrations");
    }
}
