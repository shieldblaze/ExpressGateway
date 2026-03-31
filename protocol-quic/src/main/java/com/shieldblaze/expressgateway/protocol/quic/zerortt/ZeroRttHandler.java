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
package com.shieldblaze.expressgateway.protocol.quic.zerortt;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.Objects;
import java.util.Set;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Handles 0-RTT (early data) acceptance with replay protection for QUIC/HTTP3.
 *
 * <h3>RFC 9001 Section 4.7: Accepting 0-RTT</h3>
 * <p>A server SHOULD NOT process 0-RTT data if the request is not idempotent or
 * if the application protocol does not tolerate replays. This handler enforces
 * idempotency checks on the HTTP method and delegates replay detection to
 * the {@link ReplayDetector}.</p>
 *
 * <h3>Decision Flow</h3>
 * <ol>
 *   <li>Check if 0-RTT is globally enabled</li>
 *   <li>Check if the request method is idempotent (GET, HEAD, OPTIONS, TRACE per RFC 9110)</li>
 *   <li>Check rate limit (optional, configurable max 0-RTT acceptances per window)</li>
 *   <li>Check replay detector for the 0-RTT identifier</li>
 *   <li>If all pass: accept 0-RTT data, register identifier</li>
 *   <li>If any fail: reject 0-RTT, fall back to 1-RTT</li>
 * </ol>
 *
 * <h3>Fallback Behavior</h3>
 * <p>When 0-RTT is rejected, the QUIC implementation falls back to 1-RTT. The client
 * resends the request after the full handshake completes. This is transparent to the
 * application layer -- the only effect is higher latency for rejected 0-RTT requests.</p>
 *
 * <h3>Thread Safety</h3>
 * <p>Thread-safe via atomic counters and the lock-free {@link ReplayDetector}.</p>
 */
public final class ZeroRttHandler {

    private static final Logger logger = LogManager.getLogger(ZeroRttHandler.class);

    /**
     * RFC 8470 Section 2.1: Safe methods for 0-RTT.
     * Only safe methods (those that do not cause side effects) should be accepted
     * via 0-RTT. PUT and DELETE are idempotent but NOT safe -- they modify server
     * state and their replay could have unintended consequences. Per RFC 9110 Section 9.2.1,
     * safe methods are: GET, HEAD, OPTIONS, TRACE.
     */
    private static final Set<String> SAFE_METHODS = Set.of(
            "GET", "HEAD", "OPTIONS", "TRACE"
    );

    /**
     * Result of a 0-RTT acceptance decision.
     */
    public enum ZeroRttDecision {
        /** 0-RTT data accepted -- process early data. */
        ACCEPT,
        /** 0-RTT rejected: not an idempotent method. */
        REJECT_NON_IDEMPOTENT,
        /** 0-RTT rejected: replay detected. */
        REJECT_REPLAY,
        /** 0-RTT rejected: rate limit exceeded. */
        REJECT_RATE_LIMITED,
        /** 0-RTT rejected: feature disabled. */
        REJECT_DISABLED
    }

    private final ReplayDetector replayDetector;
    private final boolean enabled;

    /**
     * Maximum 0-RTT acceptances per rate window. 0 = unlimited.
     */
    private final long maxAcceptancesPerWindow;

    /**
     * Rate window in nanoseconds.
     */
    private final long rateWindowNanos;

    /**
     * Sliding window counter for rate limiting.
     */
    private final AtomicLong windowAcceptances = new AtomicLong();
    private volatile long windowStartNanos;

    private final AtomicLong totalDecisions = new AtomicLong();
    private final AtomicLong accepted = new AtomicLong();
    private final AtomicLong rejectedNonIdempotent = new AtomicLong();
    private final AtomicLong rejectedReplay = new AtomicLong();
    private final AtomicLong rejectedRateLimited = new AtomicLong();

    /**
     * Create a ZeroRttHandler with default settings.
     *
     * @param enabled        whether 0-RTT acceptance is enabled
     * @param replayDetector the replay detector to use
     */
    public ZeroRttHandler(boolean enabled, ReplayDetector replayDetector) {
        this(enabled, replayDetector, 0, 10_000_000_000L);
    }

    /**
     * Create a ZeroRttHandler with rate limiting.
     *
     * @param enabled                whether 0-RTT acceptance is enabled
     * @param replayDetector         the replay detector
     * @param maxAcceptancesPerWindow max 0-RTT accepts per window (0 = unlimited)
     * @param rateWindowNanos        rate window in nanoseconds
     */
    public ZeroRttHandler(boolean enabled, ReplayDetector replayDetector,
                           long maxAcceptancesPerWindow, long rateWindowNanos) {
        this.enabled = enabled;
        this.replayDetector = Objects.requireNonNull(replayDetector, "replayDetector");
        this.maxAcceptancesPerWindow = maxAcceptancesPerWindow;
        this.rateWindowNanos = rateWindowNanos;
        this.windowStartNanos = System.nanoTime();
    }

    /**
     * Decide whether to accept 0-RTT data for a request.
     *
     * @param httpMethod    the HTTP method (e.g., "GET", "POST")
     * @param zeroRttIdentifier the unique identifier for this 0-RTT attempt
     *                           (see {@link ReplayDetector#computeIdentifier})
     * @return the decision
     */
    public ZeroRttDecision decide(String httpMethod, long zeroRttIdentifier) {
        totalDecisions.incrementAndGet();

        if (!enabled) {
            return ZeroRttDecision.REJECT_DISABLED;
        }

        // Check safety: try case-sensitive first to avoid toUpperCase allocation
        if (httpMethod == null || (!SAFE_METHODS.contains(httpMethod) && !SAFE_METHODS.contains(httpMethod.toUpperCase()))) {
            rejectedNonIdempotent.incrementAndGet();
            if (logger.isDebugEnabled()) {
                logger.debug("0-RTT rejected: non-safe method '{}'", httpMethod);
            }
            return ZeroRttDecision.REJECT_NON_IDEMPOTENT;
        }

        // Check replay
        if (replayDetector.isReplay(zeroRttIdentifier)) {
            rejectedReplay.incrementAndGet();
            logger.warn("0-RTT replay detected for identifier {}", zeroRttIdentifier);
            return ZeroRttDecision.REJECT_REPLAY;
        }

        // Check rate limit and atomically reserve a slot inside the synchronized
        // method to prevent TOCTOU: two threads must not both pass the check at
        // count N-1 and both increment to N and N+1.
        if (maxAcceptancesPerWindow > 0 && !tryReserveSlot()) {
            rejectedRateLimited.incrementAndGet();
            if (logger.isDebugEnabled()) {
                logger.debug("0-RTT rejected: rate limit exceeded");
            }
            return ZeroRttDecision.REJECT_RATE_LIMITED;
        }

        // Accept
        accepted.incrementAndGet();

        if (logger.isDebugEnabled()) {
            logger.debug("0-RTT accepted for method '{}', identifier {}", httpMethod, zeroRttIdentifier);
        }
        return ZeroRttDecision.ACCEPT;
    }

    /**
     * Returns the replay detector for direct access.
     */
    public ReplayDetector replayDetector() {
        return replayDetector;
    }

    public boolean isEnabled() {
        return enabled;
    }

    public long totalDecisions() {
        return totalDecisions.get();
    }

    public long accepted() {
        return accepted.get();
    }

    public long rejectedNonIdempotent() {
        return rejectedNonIdempotent.get();
    }

    public long rejectedReplay() {
        return rejectedReplay.get();
    }

    public long rejectedRateLimited() {
        return rejectedRateLimited.get();
    }

    /**
     * Lock object for rate limiter window rotation.
     * The windowStartNanos and windowAcceptances must be updated atomically together
     * to prevent TOCTOU where two threads both see an expired window and both reset.
     */
    private final Object rateLimitLock = new Object();

    /**
     * Atomically check if a slot is available and reserve it.
     * Returns {@code true} if the slot was reserved (request may proceed),
     * {@code false} if the rate limit is exhausted.
     * The check-and-increment is inside the lock to prevent the TOCTOU race where
     * two threads both pass the {@code count < max} check and both increment past max.
     */
    private boolean tryReserveSlot() {
        synchronized (rateLimitLock) {
            long now = System.nanoTime();
            if (now - windowStartNanos > rateWindowNanos) {
                // Window expired -- reset and grant this slot
                windowAcceptances.set(1);
                windowStartNanos = now;
                return true;
            }
            if (windowAcceptances.get() >= maxAcceptancesPerWindow) {
                return false;
            }
            windowAcceptances.incrementAndGet();
            return true;
        }
    }
}
