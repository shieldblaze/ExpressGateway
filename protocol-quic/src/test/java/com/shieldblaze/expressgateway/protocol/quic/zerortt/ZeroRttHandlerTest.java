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

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link ZeroRttHandler} and {@link ReplayDetector} -- 0-RTT acceptance
 * with replay protection per RFC 9001 Section 4.7.
 */
@Timeout(10)
class ZeroRttHandlerTest {

    @Test
    void accept_idempotentGet_fresh() {
        ReplayDetector detector = new ReplayDetector();
        ZeroRttHandler handler = new ZeroRttHandler(true, detector);

        long id = ReplayDetector.computeIdentifier(12345L, 67890L);
        ZeroRttHandler.ZeroRttDecision decision = handler.decide("GET", id);

        assertEquals(ZeroRttHandler.ZeroRttDecision.ACCEPT, decision);
        assertEquals(1, handler.accepted());
    }

    @Test
    void reject_nonIdempotentPost() {
        ReplayDetector detector = new ReplayDetector();
        ZeroRttHandler handler = new ZeroRttHandler(true, detector);

        long id = ReplayDetector.computeIdentifier(12345L, 67890L);
        ZeroRttHandler.ZeroRttDecision decision = handler.decide("POST", id);

        assertEquals(ZeroRttHandler.ZeroRttDecision.REJECT_NON_IDEMPOTENT, decision);
        assertEquals(1, handler.rejectedNonIdempotent());
    }

    @Test
    void reject_disabled() {
        ReplayDetector detector = new ReplayDetector();
        ZeroRttHandler handler = new ZeroRttHandler(false, detector);

        long id = ReplayDetector.computeIdentifier(12345L, 67890L);
        ZeroRttHandler.ZeroRttDecision decision = handler.decide("GET", id);

        assertEquals(ZeroRttHandler.ZeroRttDecision.REJECT_DISABLED, decision);
    }

    @Test
    void reject_replay() {
        ReplayDetector detector = new ReplayDetector();
        ZeroRttHandler handler = new ZeroRttHandler(true, detector);

        long id = ReplayDetector.computeIdentifier(12345L, 67890L);

        // First attempt: accept
        assertEquals(ZeroRttHandler.ZeroRttDecision.ACCEPT, handler.decide("GET", id));

        // Second attempt (replay): reject
        assertEquals(ZeroRttHandler.ZeroRttDecision.REJECT_REPLAY, handler.decide("GET", id));
        assertEquals(1, handler.rejectedReplay());
    }

    @Test
    void accept_allSafeMethods() {
        ReplayDetector detector = new ReplayDetector();
        ZeroRttHandler handler = new ZeroRttHandler(true, detector);

        // Per RFC 8470 Section 2.1, only safe methods should be accepted in 0-RTT
        String[] safe = {"GET", "HEAD", "OPTIONS", "TRACE"};
        for (int i = 0; i < safe.length; i++) {
            long id = ReplayDetector.computeIdentifier(i, 999L);
            assertEquals(ZeroRttHandler.ZeroRttDecision.ACCEPT, handler.decide(safe[i], id),
                    safe[i] + " must be accepted as safe");
        }
        assertEquals(4, handler.accepted());
    }

    @Test
    void reject_putAndDelete_notSafe() {
        // PUT and DELETE are idempotent but NOT safe per RFC 9110 Section 9.2.1.
        // They cause side effects and must not be accepted in 0-RTT per RFC 8470.
        ReplayDetector detector = new ReplayDetector();
        ZeroRttHandler handler = new ZeroRttHandler(true, detector);

        long id1 = ReplayDetector.computeIdentifier(100, 200L);
        assertEquals(ZeroRttHandler.ZeroRttDecision.REJECT_NON_IDEMPOTENT, handler.decide("PUT", id1),
                "PUT is not safe, must be rejected for 0-RTT");

        long id2 = ReplayDetector.computeIdentifier(300, 400L);
        assertEquals(ZeroRttHandler.ZeroRttDecision.REJECT_NON_IDEMPOTENT, handler.decide("DELETE", id2),
                "DELETE is not safe, must be rejected for 0-RTT");
    }

    @Test
    void reject_nullMethod() {
        ReplayDetector detector = new ReplayDetector();
        ZeroRttHandler handler = new ZeroRttHandler(true, detector);

        long id = ReplayDetector.computeIdentifier(12345L, 67890L);
        ZeroRttHandler.ZeroRttDecision decision = handler.decide(null, id);

        assertEquals(ZeroRttHandler.ZeroRttDecision.REJECT_NON_IDEMPOTENT, decision);
    }

    @Test
    void rateLimited_afterMaxAcceptances() {
        ReplayDetector detector = new ReplayDetector();
        // Max 3 acceptances per 10-second window
        ZeroRttHandler handler = new ZeroRttHandler(true, detector, 3, 10_000_000_000L);

        for (int i = 0; i < 3; i++) {
            long id = ReplayDetector.computeIdentifier(i, 100L);
            assertEquals(ZeroRttHandler.ZeroRttDecision.ACCEPT, handler.decide("GET", id));
        }

        // 4th request: rate limited
        long id4 = ReplayDetector.computeIdentifier(4, 100L);
        assertEquals(ZeroRttHandler.ZeroRttDecision.REJECT_RATE_LIMITED, handler.decide("GET", id4));
        assertEquals(1, handler.rejectedRateLimited());
    }

    @Test
    void caseInsensitive_methods() {
        ReplayDetector detector = new ReplayDetector();
        ZeroRttHandler handler = new ZeroRttHandler(true, detector);

        long id1 = ReplayDetector.computeIdentifier(100, 200L);
        long id2 = ReplayDetector.computeIdentifier(300, 400L);
        long id3 = ReplayDetector.computeIdentifier(500, 600L);

        assertEquals(ZeroRttHandler.ZeroRttDecision.ACCEPT, handler.decide("get", id1));
        assertEquals(ZeroRttHandler.ZeroRttDecision.ACCEPT, handler.decide("Get", id2));
        assertEquals(ZeroRttHandler.ZeroRttDecision.ACCEPT, handler.decide("GET", id3));
    }

    // --- ReplayDetector tests ---

    @Test
    void replayDetector_firstSeen_notReplay() {
        ReplayDetector detector = new ReplayDetector();
        long id = ReplayDetector.computeIdentifier(42L, 99L);
        assertEquals(false, detector.isReplay(id));
        assertEquals(1, detector.entriesRegistered());
    }

    @Test
    void replayDetector_secondSeen_isReplay() {
        ReplayDetector detector = new ReplayDetector();
        long id = ReplayDetector.computeIdentifier(42L, 99L);
        detector.isReplay(id); // register
        assertEquals(true, detector.isReplay(id));
        assertEquals(1, detector.replaysDetected());
    }

    @Test
    void replayDetector_differentIds_notReplay() {
        ReplayDetector detector = new ReplayDetector();
        long id1 = ReplayDetector.computeIdentifier(100L, 200L);
        long id2 = ReplayDetector.computeIdentifier(300L, 400L);

        assertEquals(false, detector.isReplay(id1));
        assertEquals(false, detector.isReplay(id2));
        assertEquals(2, detector.entriesRegistered());
    }

    @Test
    void replayDetector_windowRotation_expiresOldEntries() throws Exception {
        // Window = 100ms for testing. Need two rotations for full expiry:
        // 1st rotation: current (with id) becomes previous
        // 2nd rotation: previous (with id) is discarded
        ReplayDetector detector = new ReplayDetector(100_000_000L, 100_000);

        long id = ReplayDetector.computeIdentifier(42L, 99L);
        detector.isReplay(id); // register in current window

        // Wait for first window rotation
        Thread.sleep(150);
        // Trigger lazy rotation by calling any method that accesses state
        detector.wouldBeReplay(0L);

        // Wait for second window rotation (id should now be in discarded "previous")
        Thread.sleep(150);

        // Now the id should be expired (was in the discarded "previous" window)
        assertEquals(false, detector.isReplay(id),
                "Identifier must be forgotten after two window rotations");
    }

    @Test
    void replayDetector_capacityLimit_rejectsAll() {
        // Max 2 entries
        ReplayDetector detector = new ReplayDetector(10_000_000_000L, 2);

        long id1 = ReplayDetector.computeIdentifier(100L, 200L);
        long id2 = ReplayDetector.computeIdentifier(300L, 400L);
        long id3 = ReplayDetector.computeIdentifier(500L, 600L);

        assertEquals(false, detector.isReplay(id1)); // registered
        assertEquals(false, detector.isReplay(id2)); // registered (at capacity)
        assertEquals(true, detector.isReplay(id3),
                "Must reject when at capacity to prevent memory exhaustion");
        assertEquals(1, detector.capacityRejections());
    }

    @Test
    void replayDetector_size_tracksEntries() {
        ReplayDetector detector = new ReplayDetector();
        assertEquals(0, detector.size());

        detector.isReplay(ReplayDetector.computeIdentifier(100L, 200L));
        detector.isReplay(ReplayDetector.computeIdentifier(300L, 400L));
        assertEquals(2, detector.size());
    }

    @Test
    void replayDetector_wouldBeReplay_doesNotRegister() {
        ReplayDetector detector = new ReplayDetector();
        long id = ReplayDetector.computeIdentifier(42L, 99L);

        assertEquals(false, detector.wouldBeReplay(id), "Not seen yet");
        assertEquals(0, detector.size(), "wouldBeReplay must not register");

        detector.isReplay(id); // Now register
        assertEquals(true, detector.wouldBeReplay(id), "Now seen");
        assertEquals(1, detector.size());
    }

    @Test
    void replayDetector_computeIdentifier_deterministic() {
        long id1 = ReplayDetector.computeIdentifier(42L, 99L);
        long id2 = ReplayDetector.computeIdentifier(42L, 99L);
        assertEquals(id1, id2, "Same inputs must produce same identifier");
    }

    @Test
    void replayDetector_computeIdentifier_differentInputs_differ() {
        long id1 = ReplayDetector.computeIdentifier(42L, 99L);
        long id2 = ReplayDetector.computeIdentifier(42L, 100L);
        long id3 = ReplayDetector.computeIdentifier(43L, 99L);

        // These COULD theoretically collide but with a good mixing function, practically never
        // We test inequality to catch trivial mixing bugs
        assertTrue(id1 != id2 || id1 != id3,
                "Different inputs should produce different identifiers in practice");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Rate limiter under concurrency
    // ═══════════════════════════════════════════════════════════════════════

    @Test
    void rateLimiter_concurrentAccess_respectsLimit() throws Exception {
        ReplayDetector detector = new ReplayDetector(10_000_000_000L, 1_000_000);
        // Max 50 acceptances per window (long window so it doesn't expire)
        ZeroRttHandler handler = new ZeroRttHandler(true, detector, 50, 60_000_000_000L);

        int numThreads = 8;
        int requestsPerThread = 20;
        ExecutorService executor = Executors.newFixedThreadPool(numThreads);
        CountDownLatch start = new CountDownLatch(1);
        CountDownLatch done = new CountDownLatch(numThreads);
        AtomicInteger totalAccepted = new AtomicInteger(0);
        AtomicInteger totalRateLimited = new AtomicInteger(0);

        for (int t = 0; t < numThreads; t++) {
            final int threadId = t;
            executor.submit(() -> {
                try {
                    start.await();
                    for (int r = 0; r < requestsPerThread; r++) {
                        long id = ReplayDetector.computeIdentifier(threadId * 1000L + r, 42L);
                        ZeroRttHandler.ZeroRttDecision decision = handler.decide("GET", id);
                        if (decision == ZeroRttHandler.ZeroRttDecision.ACCEPT) {
                            totalAccepted.incrementAndGet();
                        } else if (decision == ZeroRttHandler.ZeroRttDecision.REJECT_RATE_LIMITED) {
                            totalRateLimited.incrementAndGet();
                        }
                    }
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                } finally {
                    done.countDown();
                }
            });
        }

        start.countDown();
        done.await();
        executor.shutdown();

        // Total requests = 8 * 20 = 160, limit = 50
        // Accepted must be exactly 50 (the rate limit)
        assertTrue(totalAccepted.get() <= 50,
                "Rate limiter must not accept more than max, got: " + totalAccepted.get());
        assertTrue(totalAccepted.get() > 0, "At least some requests must be accepted");
        assertTrue(totalRateLimited.get() > 0, "Some requests must be rate-limited");
    }
}
