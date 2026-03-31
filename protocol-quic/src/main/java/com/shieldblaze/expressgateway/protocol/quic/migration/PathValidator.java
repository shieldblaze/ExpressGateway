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

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.net.InetSocketAddress;
import java.security.SecureRandom;
import java.util.Arrays;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

/**
 * PATH_CHALLENGE / PATH_RESPONSE validation for QUIC connection migration.
 *
 * <h3>RFC 9000 Section 8.2: Path Validation</h3>
 * <p>Before an endpoint can use a new path, it MUST validate the path by sending a
 * PATH_CHALLENGE frame containing 8 bytes of unpredictable data. The peer responds
 * with a PATH_RESPONSE frame echoing the same 8 bytes. Path validation is considered
 * successful only when the correct PATH_RESPONSE is received.</p>
 *
 * <h3>Security Properties</h3>
 * <ul>
 *   <li>Challenge data is cryptographically random (8 bytes per RFC 9000 Section 19.17)</li>
 *   <li>Pending challenges are keyed by remote address to prevent cross-path confusion</li>
 *   <li>Challenges expire after a configurable timeout to prevent memory exhaustion</li>
 *   <li>Constant-time comparison prevents timing attacks on challenge data</li>
 * </ul>
 *
 * <h3>Thread Safety</h3>
 * <p>All operations use {@link ConcurrentHashMap} for lock-free concurrent access
 * from multiple EventLoop threads.</p>
 */
public final class PathValidator {

    private static final Logger logger = LogManager.getLogger(PathValidator.class);

    /**
     * PATH_CHALLENGE data is exactly 8 bytes per RFC 9000 Section 19.17.
     */
    static final int CHALLENGE_DATA_LENGTH = 8;

    /**
     * Default timeout for pending path challenges in nanoseconds.
     * RFC 9000 does not specify a timeout, but we use a sensible default to bound memory.
     */
    private static final long DEFAULT_CHALLENGE_TIMEOUT_NANOS = 10_000_000_000L; // 10 seconds

    /**
     * A pending path validation challenge.
     *
     * @param challengeData the 8-byte challenge data sent in PATH_CHALLENGE
     * @param createdAtNanos timestamp when the challenge was issued
     * @param address the remote address being validated
     */
    public record PendingChallenge(byte[] challengeData, long createdAtNanos, InetSocketAddress address) {

        /**
         * Check if this challenge has expired.
         *
         * @param nowNanos current nanoTime
         * @param timeoutNanos timeout duration in nanos
         * @return true if expired
         */
        public boolean isExpired(long nowNanos, long timeoutNanos) {
            return nowNanos - createdAtNanos > timeoutNanos;
        }
    }

    /**
     * Result of a path validation response check.
     */
    public enum ValidationResult {
        /** PATH_RESPONSE matched a pending challenge -- path is validated. */
        VALIDATED,
        /** PATH_RESPONSE did not match any pending challenge. */
        NO_MATCH,
        /** PATH_RESPONSE matched but the challenge had expired. */
        EXPIRED
    }

    private final ConcurrentHashMap<InetSocketAddress, PendingChallenge> pendingChallenges = new ConcurrentHashMap<>();
    private final long challengeTimeoutNanos;
    private final ThreadLocal<SecureRandom> threadLocalRandom = ThreadLocal.withInitial(SecureRandom::new);
    private final AtomicLong challengesSent = new AtomicLong();
    private final AtomicLong challengesValidated = new AtomicLong();
    private final AtomicLong challengesFailed = new AtomicLong();

    /**
     * Create a path validator with default timeout (10 seconds).
     */
    public PathValidator() {
        this(DEFAULT_CHALLENGE_TIMEOUT_NANOS);
    }

    /**
     * Create a path validator with a custom timeout.
     *
     * @param challengeTimeoutNanos timeout for pending challenges in nanoseconds
     */
    public PathValidator(long challengeTimeoutNanos) {
        if (challengeTimeoutNanos <= 0) {
            throw new IllegalArgumentException("Challenge timeout must be positive, got: " + challengeTimeoutNanos);
        }
        this.challengeTimeoutNanos = challengeTimeoutNanos;
    }

    /**
     * Generate a PATH_CHALLENGE for a new remote address.
     *
     * <p>Creates 8 bytes of cryptographically random data and registers it
     * as a pending challenge for the given address.</p>
     *
     * @param remoteAddress the address to validate
     * @return the 8-byte challenge data to send in a PATH_CHALLENGE frame
     */
    public byte[] initiateChallenge(InetSocketAddress remoteAddress) {
        byte[] challengeData = new byte[CHALLENGE_DATA_LENGTH];
        threadLocalRandom.get().nextBytes(challengeData);

        PendingChallenge challenge = new PendingChallenge(
                challengeData, System.nanoTime(), remoteAddress);

        // Replace any existing pending challenge for this address.
        // Only one outstanding challenge per address is needed.
        pendingChallenges.put(remoteAddress, challenge);
        challengesSent.incrementAndGet();

        if (logger.isDebugEnabled()) {
            logger.debug("Initiated PATH_CHALLENGE for address {}", remoteAddress);
        }

        return challengeData;
    }

    /**
     * Validate a PATH_RESPONSE against pending challenges.
     *
     * @param remoteAddress the address that sent the PATH_RESPONSE
     * @param responseData  the 8-byte data from the PATH_RESPONSE frame
     * @return the validation result
     */
    public ValidationResult validateResponse(InetSocketAddress remoteAddress, byte[] responseData) {
        if (responseData == null || responseData.length != CHALLENGE_DATA_LENGTH) {
            challengesFailed.incrementAndGet();
            return ValidationResult.NO_MATCH;
        }

        PendingChallenge pending = pendingChallenges.remove(remoteAddress);
        if (pending == null) {
            challengesFailed.incrementAndGet();
            return ValidationResult.NO_MATCH;
        }

        // Check expiry before comparing data
        if (pending.isExpired(System.nanoTime(), challengeTimeoutNanos)) {
            challengesFailed.incrementAndGet();
            if (logger.isDebugEnabled()) {
                logger.debug("PATH_RESPONSE for {} arrived after timeout", remoteAddress);
            }
            return ValidationResult.EXPIRED;
        }

        // Constant-time comparison to prevent timing attacks
        if (constantTimeEquals(pending.challengeData(), responseData)) {
            challengesValidated.incrementAndGet();
            if (logger.isDebugEnabled()) {
                logger.debug("PATH_RESPONSE validated for address {}", remoteAddress);
            }
            return ValidationResult.VALIDATED;
        }

        challengesFailed.incrementAndGet();
        logger.warn("PATH_RESPONSE data mismatch for address {}", remoteAddress);
        return ValidationResult.NO_MATCH;
    }

    /**
     * Check if a path validation is currently pending for an address.
     *
     * @param remoteAddress the address to check
     * @return true if a PATH_CHALLENGE is outstanding
     */
    public boolean hasPendingChallenge(InetSocketAddress remoteAddress) {
        PendingChallenge pending = pendingChallenges.get(remoteAddress);
        if (pending == null) {
            return false;
        }
        // Check if expired; if so, clean up lazily
        if (pending.isExpired(System.nanoTime(), challengeTimeoutNanos)) {
            pendingChallenges.remove(remoteAddress, pending);
            return false;
        }
        return true;
    }

    /**
     * Cancel a pending path challenge for an address.
     *
     * @param remoteAddress the address whose challenge to cancel
     */
    public void cancelChallenge(InetSocketAddress remoteAddress) {
        pendingChallenges.remove(remoteAddress);
    }

    /**
     * Remove all expired pending challenges. Called periodically by external eviction.
     *
     * @return the number of expired challenges removed
     */
    public int evictExpired() {
        long now = System.nanoTime();
        int[] evicted = {0};
        pendingChallenges.entrySet().removeIf(e -> {
            if (e.getValue().isExpired(now, challengeTimeoutNanos)) {
                evicted[0]++;
                return true;
            }
            return false;
        });
        return evicted[0];
    }

    /**
     * Returns the number of pending (unresolved) path challenges.
     */
    public int pendingCount() {
        return pendingChallenges.size();
    }

    public long challengesSent() {
        return challengesSent.get();
    }

    public long challengesValidated() {
        return challengesValidated.get();
    }

    public long challengesFailed() {
        return challengesFailed.get();
    }

    private static boolean constantTimeEquals(byte[] a, byte[] b) {
        if (a.length != b.length) return false;
        int result = 0;
        for (int i = 0; i < a.length; i++) {
            result |= a[i] ^ b[i];
        }
        return result == 0;
    }
}
