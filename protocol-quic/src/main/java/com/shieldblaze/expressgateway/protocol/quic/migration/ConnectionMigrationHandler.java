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

import com.shieldblaze.expressgateway.protocol.quic.QuicBackendSession;
import com.shieldblaze.expressgateway.protocol.quic.QuicCidSessionMap;
import com.shieldblaze.expressgateway.protocol.quic.nat.NatRebindingDetector;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.util.Arrays;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Handles QUIC connection migration per RFC 9000 Section 9.
 *
 * <h3>Connection Migration</h3>
 * <p>QUIC connections are identified by Connection IDs, not the 4-tuple. When a client's
 * network path changes (e.g., WiFi to cellular), the source address changes but the CID
 * remains the same. The proxy detects this address change, initiates path validation,
 * and updates routing tables upon successful validation.</p>
 *
 * <h3>Anti-Amplification (RFC 9000 Section 8.1, 9.3)</h3>
 * <p>Before path validation completes, the server MUST limit data sent to the new address
 * to at most three times the data received from that address. This prevents the proxy
 * from being used as a traffic amplifier against a spoofed address.</p>
 *
 * <h3>Rate Limiting</h3>
 * <p>Migration attempts from the same original address are rate-limited to prevent
 * resource exhaustion from rapid migrations (e.g., an attacker triggering continuous
 * migration events).</p>
 *
 * <h3>Integration with NAT Rebinding</h3>
 * <p>When only the port changes (same IP), the {@link NatRebindingDetector} classifies
 * this as NAT rebinding rather than full migration, allowing a lighter-weight validation
 * path.</p>
 *
 * <h3>Thread Safety</h3>
 * <p>All state is stored in {@link ConcurrentHashMap} structures. The handler is safe
 * for concurrent access from multiple EventLoop threads (SO_REUSEPORT multi-bind).</p>
 */
public final class ConnectionMigrationHandler {

    private static final Logger logger = LogManager.getLogger(ConnectionMigrationHandler.class);

    /**
     * State of a migration in progress.
     */
    public enum MigrationState {
        /** Migration detected, path validation initiated. */
        VALIDATING,
        /** Path validation succeeded, routing tables updated. */
        COMPLETED,
        /** Path validation failed or timed out. */
        FAILED,
        /** Migration rejected due to rate limiting or anti-amplification. */
        REJECTED
    }

    /**
     * A migration event tracking the transition from old to new address.
     *
     * @param oldAddress the address before migration
     * @param newAddress the address after migration
     * @param dcid the connection ID used for routing
     * @param state the current state of the migration
     * @param bytesReceivedFromNew bytes received from the new address (for anti-amplification)
     * @param bytesSentToNew bytes sent to the new address (for anti-amplification)
     * @param createdAtNanos when the migration was detected
     */
    public record MigrationEvent(
            InetSocketAddress oldAddress,
            InetSocketAddress newAddress,
            byte[] dcid,
            MigrationState state,
            long bytesReceivedFromNew,
            long bytesSentToNew,
            long createdAtNanos
    ) {
        /**
         * Check anti-amplification limit: server MUST NOT send more than 3x received.
         * RFC 9000 Section 8.1.
         */
        public boolean canSendBytes(long additionalBytes) {
            return bytesSentToNew + additionalBytes <= bytesReceivedFromNew * 3;
        }

        MigrationEvent withState(MigrationState newState) {
            return new MigrationEvent(oldAddress, newAddress, dcid, newState,
                    bytesReceivedFromNew, bytesSentToNew, createdAtNanos);
        }

        MigrationEvent withReceivedBytes(long newBytesReceived) {
            return new MigrationEvent(oldAddress, newAddress, dcid, state,
                    bytesReceivedFromNew + newBytesReceived, bytesSentToNew, createdAtNanos);
        }

        MigrationEvent withSentBytes(long newBytesSent) {
            return new MigrationEvent(oldAddress, newAddress, dcid, state,
                    bytesReceivedFromNew, bytesSentToNew + newBytesSent, createdAtNanos);
        }
    }

    private final PathValidator pathValidator;
    private final NatRebindingDetector natRebindingDetector;
    private final Map<InetSocketAddress, QuicBackendSession> addressSessionMap;
    private final QuicCidSessionMap cidSessionMap;

    /**
     * Composite key for active migrations: DCID + new address.
     * Using only the new address is insufficient because the same address could be
     * the target of migrations from different connections simultaneously.
     */
    public record MigrationKey(InetSocketAddress newAddress, byte[] dcid) {
        @Override
        public boolean equals(Object o) {
            if (this == o) return true;
            if (!(o instanceof MigrationKey that)) return false;
            return Objects.equals(newAddress, that.newAddress) && Arrays.equals(dcid, that.dcid);
        }

        @Override
        public int hashCode() {
            return Objects.hash(newAddress, Arrays.hashCode(dcid));
        }
    }

    /**
     * Active migrations keyed by composite key (DCID + new address).
     * Allows tracking anti-amplification limits and migration state per connection+address.
     */
    private final ConcurrentHashMap<MigrationKey, MigrationEvent> activeMigrations = new ConcurrentHashMap<>();

    /**
     * Secondary index: new address -> composite migration key.
     * Eliminates O(n) linear scans in tryConsumeAmplificationBudget(), canSendTo(),
     * trackBytesReceived(), isMigrating(), and getMigrationEvent() which all need
     * to find an active migration by the new address.
     */
    private final ConcurrentHashMap<InetSocketAddress, MigrationKey> addressToMigrationKey = new ConcurrentHashMap<>();

    /**
     * Rate limiter: tracks migration attempts per original address.
     * Key: original (pre-migration) address, Value: timestamp of last migration attempt.
     */
    private final ConcurrentHashMap<InetSocketAddress, Long> migrationAttemptTimestamps = new ConcurrentHashMap<>();

    /**
     * Minimum interval between migration attempts from the same source in nanoseconds.
     * Default: 1 second. Prevents rapid migration storms.
     */
    private final long migrationRateLimitNanos;

    private final AtomicLong totalMigrations = new AtomicLong();
    private final AtomicLong successfulMigrations = new AtomicLong();
    private final AtomicLong rejectedMigrations = new AtomicLong();
    private final AtomicLong natRebindings = new AtomicLong();

    /**
     * Create a new connection migration handler.
     *
     * @param pathValidator    the path validator for PATH_CHALLENGE/PATH_RESPONSE
     * @param natRebindingDetector the NAT rebinding detector for port-only changes
     * @param addressSessionMap the address-to-session map from QuicProxyHandler
     * @param cidSessionMap     the CID-to-session map
     */
    public ConnectionMigrationHandler(PathValidator pathValidator,
                                       NatRebindingDetector natRebindingDetector,
                                       Map<InetSocketAddress, QuicBackendSession> addressSessionMap,
                                       QuicCidSessionMap cidSessionMap) {
        this(pathValidator, natRebindingDetector, addressSessionMap, cidSessionMap, 1_000_000_000L);
    }

    /**
     * Create a new connection migration handler with custom rate limit.
     *
     * @param pathValidator    the path validator
     * @param natRebindingDetector the NAT rebinding detector
     * @param addressSessionMap the address-to-session map
     * @param cidSessionMap     the CID-to-session map
     * @param migrationRateLimitNanos minimum interval between migrations in nanos
     */
    public ConnectionMigrationHandler(PathValidator pathValidator,
                                       NatRebindingDetector natRebindingDetector,
                                       Map<InetSocketAddress, QuicBackendSession> addressSessionMap,
                                       QuicCidSessionMap cidSessionMap,
                                       long migrationRateLimitNanos) {
        this.pathValidator = Objects.requireNonNull(pathValidator, "pathValidator");
        this.natRebindingDetector = Objects.requireNonNull(natRebindingDetector, "natRebindingDetector");
        this.addressSessionMap = Objects.requireNonNull(addressSessionMap, "addressSessionMap");
        this.cidSessionMap = Objects.requireNonNull(cidSessionMap, "cidSessionMap");
        this.migrationRateLimitNanos = migrationRateLimitNanos;
    }

    /**
     * Result of a migration initiation, containing challenge data for both old and new paths.
     *
     * @param newPathChallenge the 8-byte PATH_CHALLENGE for the new address
     * @param oldPathChallenge the 8-byte PATH_CHALLENGE for the old address (RFC 9000 Section 9.3.2),
     *                         or null if the old address is null
     */
    public record MigrationChallenges(byte[] newPathChallenge, byte[] oldPathChallenge) {
    }

    /**
     * Handle a potential connection migration. Called when a packet arrives from a new
     * address with a CID that maps to an existing session.
     *
     * <p>This method determines whether the address change is:
     * <ul>
     *   <li>NAT rebinding (port-only change) -- lightweight validation</li>
     *   <li>Full migration (IP change) -- full PATH_CHALLENGE validation</li>
     * </ul></p>
     *
     * <p>Per RFC 9000 Section 9.3.2, after detecting a migration, the server SHOULD
     * also send a PATH_CHALLENGE to the old address to verify the old path is still
     * valid (to detect potential on-path attacks).</p>
     *
     * @param oldAddress the previous client address for this CID
     * @param newAddress the new client address from the incoming packet
     * @param dcid       the DCID from the packet
     * @param session    the existing backend session for this CID
     * @param packetSize size of the incoming packet (for anti-amplification)
     * @return the challenge data to send (PATH_CHALLENGE), or null if rejected/NAT rebinding handled
     */
    public byte[] handleMigration(InetSocketAddress oldAddress, InetSocketAddress newAddress,
                                   byte[] dcid, QuicBackendSession session, int packetSize) {
        totalMigrations.incrementAndGet();

        // Check if this is NAT rebinding (port-only change)
        if (natRebindingDetector.isNatRebinding(oldAddress, newAddress)) {
            natRebindings.incrementAndGet();
            // NAT rebinding: update address mapping without full path validation
            natRebindingDetector.handleRebinding(oldAddress, newAddress, session, addressSessionMap);
            if (logger.isDebugEnabled()) {
                logger.debug("NAT rebinding handled: {} -> {} for CID session", oldAddress, newAddress);
            }
            return null;
        }

        // Rate limit migration attempts
        if (!checkMigrationRateLimit(oldAddress)) {
            rejectedMigrations.incrementAndGet();
            logger.warn("Migration rate-limited for address {}", oldAddress);
            return null;
        }

        // Full migration: initiate path validation on NEW path
        byte[] challengeData = pathValidator.initiateChallenge(newAddress);

        // RFC 9000 Section 9.3.2: also initiate PATH_CHALLENGE on OLD path
        // to verify old path is still valid (detects on-path attacks)
        if (oldAddress != null) {
            pathValidator.initiateChallenge(oldAddress);
        }

        MigrationKey migrationKey = new MigrationKey(newAddress, dcid);
        MigrationEvent event = new MigrationEvent(
                oldAddress, newAddress, dcid, MigrationState.VALIDATING,
                packetSize, 0, System.nanoTime());
        activeMigrations.put(migrationKey, event);
        addressToMigrationKey.put(newAddress, migrationKey);

        if (logger.isInfoEnabled()) {
            logger.info("Connection migration detected: {} -> {}, initiating path validation on both paths",
                    oldAddress, newAddress);
        }

        return challengeData;
    }

    /**
     * Process a PATH_RESPONSE and complete migration if validated.
     *
     * @param fromAddress  the address that sent the PATH_RESPONSE
     * @param responseData the 8-byte PATH_RESPONSE data
     * @return the migration state after processing
     */
    public MigrationState processPathResponse(InetSocketAddress fromAddress, byte[] responseData) {
        PathValidator.ValidationResult result = pathValidator.validateResponse(fromAddress, responseData);

        // O(1) lookup via secondary index instead of linear scan
        MigrationKey matchedKey = addressToMigrationKey.get(fromAddress);
        if (matchedKey == null) {
            return MigrationState.FAILED;
        }
        MigrationEvent event = activeMigrations.get(matchedKey);
        if (event == null) {
            addressToMigrationKey.remove(fromAddress);
            return MigrationState.FAILED;
        }

        return switch (result) {
            case VALIDATED -> {
                completeMigration(event);
                activeMigrations.remove(matchedKey);
                addressToMigrationKey.remove(fromAddress);
                yield MigrationState.COMPLETED;
            }
            case EXPIRED -> {
                activeMigrations.remove(matchedKey);
                addressToMigrationKey.remove(fromAddress);
                yield MigrationState.FAILED;
            }
            case NO_MATCH -> MigrationState.FAILED;
        };
    }

    /**
     * Track bytes received from a new address during migration for anti-amplification.
     *
     * @param fromAddress the address that sent data
     * @param bytesReceived the number of bytes received
     */
    public void trackBytesReceived(InetSocketAddress fromAddress, long bytesReceived) {
        MigrationKey key = addressToMigrationKey.get(fromAddress);
        if (key != null) {
            activeMigrations.computeIfPresent(key, (k, event) ->
                    event.withReceivedBytes(bytesReceived));
        }
    }

    /**
     * Atomically check anti-amplification limit AND consume budget if allowed.
     * Replaces the separate canSendTo() + trackBytesSent() pattern to eliminate
     * TOCTOU race condition.
     *
     * @param toAddress   the destination address
     * @param bytesToSend the number of bytes to send
     * @return true if sending is allowed and the budget was consumed
     */
    public boolean tryConsumeAmplificationBudget(InetSocketAddress toAddress, long bytesToSend) {
        MigrationKey key = addressToMigrationKey.get(toAddress);
        if (key == null) {
            return true; // No active migration -- no amplification limit
        }
        boolean[] allowed = {false};
        activeMigrations.computeIfPresent(key, (k, event) -> {
            if (event.canSendBytes(bytesToSend)) {
                allowed[0] = true;
                return event.withSentBytes(bytesToSend);
            }
            return event;
        });
        return allowed[0];
    }

    /**
     * Check anti-amplification limit before sending to a migrating address.
     *
     * @param toAddress      the destination address
     * @param bytesToSend    the number of bytes to send
     * @return true if sending is allowed within anti-amplification limits
     */
    public boolean canSendTo(InetSocketAddress toAddress, long bytesToSend) {
        MigrationKey key = addressToMigrationKey.get(toAddress);
        if (key == null) {
            return true; // No active migration -- no amplification limit
        }
        MigrationEvent event = activeMigrations.get(key);
        if (event == null) {
            return true;
        }
        return event.canSendBytes(bytesToSend);
    }

    /**
     * Track bytes sent to a migrating address for anti-amplification.
     *
     * @param toAddress the destination address
     * @param bytesSent the number of bytes sent
     */
    public void trackBytesSent(InetSocketAddress toAddress, long bytesSent) {
        MigrationKey key = addressToMigrationKey.get(toAddress);
        if (key != null) {
            activeMigrations.computeIfPresent(key, (k, event) ->
                    event.withSentBytes(bytesSent));
        }
    }

    /**
     * Check if a migration is in progress for the given address.
     *
     * @param address the address to check
     * @return true if a migration to this address is in progress
     */
    public boolean isMigrating(InetSocketAddress address) {
        MigrationKey key = addressToMigrationKey.get(address);
        if (key == null) {
            return false;
        }
        MigrationEvent event = activeMigrations.get(key);
        return event != null && event.state() == MigrationState.VALIDATING;
    }

    /**
     * Get the current migration event for an address, if any.
     */
    public MigrationEvent getMigrationEvent(InetSocketAddress address) {
        MigrationKey key = addressToMigrationKey.get(address);
        if (key == null) {
            return null;
        }
        return activeMigrations.get(key);
    }

    public long totalMigrations() {
        return totalMigrations.get();
    }

    public long successfulMigrations() {
        return successfulMigrations.get();
    }

    public long rejectedMigrations() {
        return rejectedMigrations.get();
    }

    public long natRebindings() {
        return natRebindings.get();
    }

    /**
     * Evict stale migration events.
     *
     * @param maxAgeNanos maximum age for active migrations
     * @return number of stale events removed
     */
    public int evictStaleMigrations(long maxAgeNanos) {
        long now = System.nanoTime();
        int[] evicted = {0};
        activeMigrations.entrySet().removeIf(e -> {
            if (now - e.getValue().createdAtNanos() > maxAgeNanos) {
                addressToMigrationKey.remove(e.getKey().newAddress());
                evicted[0]++;
                return true;
            }
            return false;
        });

        // Also evict stale rate limit entries
        migrationAttemptTimestamps.entrySet().removeIf(e ->
                now - e.getValue() > migrationRateLimitNanos * 60);

        return evicted[0];
    }

    /**
     * Returns the path validator for testing purposes.
     */
    public PathValidator pathValidator() {
        return pathValidator;
    }

    /**
     * Complete a successful migration: update address session map and CID mappings.
     */
    private void completeMigration(MigrationEvent event) {
        InetSocketAddress newAddress = event.newAddress();
        byte[] dcid = event.dcid();

        // Look up the session by CID
        QuicBackendSession session = cidSessionMap.get(dcid);
        if (session != null) {
            // Update address-to-session mapping
            addressSessionMap.put(newAddress, session);

            // Optionally remove old address mapping (the session may still
            // receive packets from the old address during migration)
            // addressSessionMap.remove(event.oldAddress());

            successfulMigrations.incrementAndGet();
            if (logger.isInfoEnabled()) {
                logger.info("Connection migration completed: {} -> {} for backend {}",
                        event.oldAddress(), newAddress, session.node().socketAddress());
            }
        } else {
            logger.warn("Migration completed but CID session not found for address {}", newAddress);
        }
    }

    /**
     * Check if a migration from the given address is allowed (rate limiting).
     * Uses compute() to atomically check and update, eliminating the TOCTOU race.
     */
    private boolean checkMigrationRateLimit(InetSocketAddress address) {
        long now = System.nanoTime();
        boolean[] allowed = {false};

        migrationAttemptTimestamps.compute(address, (addr, lastAttempt) -> {
            if (lastAttempt == null || now - lastAttempt >= migrationRateLimitNanos) {
                allowed[0] = true;
                return now;
            }
            return lastAttempt;
        });

        return allowed[0];
    }
}
