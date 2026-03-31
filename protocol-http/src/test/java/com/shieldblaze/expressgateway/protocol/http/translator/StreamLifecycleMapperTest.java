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
package com.shieldblaze.expressgateway.protocol.http.translator;

import com.shieldblaze.expressgateway.protocol.http.translator.StreamLifecycleMapper.StreamState;
import com.shieldblaze.expressgateway.protocol.http.translator.StreamLifecycleMapper.TransitionResult;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link StreamLifecycleMapper} covering the unified stream state machine
 * used for protocol translation between HTTP/1.1, HTTP/2, and HTTP/3.
 */
class StreamLifecycleMapperTest {

    private StreamLifecycleMapper mapper;

    @BeforeEach
    void setUp() {
        mapper = new StreamLifecycleMapper();
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Normal lifecycle: IDLE → OPEN → HALF_CLOSED_LOCAL → CLOSED
    // Models: H1 request sent → request complete → response complete
    //         H2 HEADERS(no ES) → DATA(ES) → response HEADERS(ES)
    //         H3 HEADERS → FIN sent → FIN received
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class NormalLifecycle {

        @Test
        void fullClientInitiatedClose() {
            int streamId = 1;
            mapper.open(streamId);
            assertEquals(StreamState.IDLE, mapper.state(streamId));

            TransitionResult r1 = mapper.activate(streamId);
            assertTrue(r1.valid());
            assertEquals(StreamState.IDLE, r1.previousState());
            assertEquals(StreamState.OPEN, r1.newState());
            assertEquals(StreamState.OPEN, mapper.state(streamId));

            TransitionResult r2 = mapper.halfCloseLocal(streamId);
            assertTrue(r2.valid());
            assertEquals(StreamState.OPEN, r2.previousState());
            assertEquals(StreamState.HALF_CLOSED_LOCAL, r2.newState());

            TransitionResult r3 = mapper.halfCloseRemote(streamId);
            assertTrue(r3.valid());
            assertEquals(StreamState.HALF_CLOSED_LOCAL, r3.previousState());
            assertEquals(StreamState.CLOSED, r3.newState());
        }

        @Test
        void fullServerInitiatedClose() {
            int streamId = 3;
            mapper.open(streamId);
            mapper.activate(streamId);

            // Remote closes first (server sent endStream in response)
            TransitionResult r1 = mapper.halfCloseRemote(streamId);
            assertTrue(r1.valid());
            assertEquals(StreamState.HALF_CLOSED_REMOTE, r1.newState());

            // Then local closes (client endStream or connection close)
            TransitionResult r2 = mapper.halfCloseLocal(streamId);
            assertTrue(r2.valid());
            assertEquals(StreamState.CLOSED, r2.newState());
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Invalid transitions
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class InvalidTransitions {

        @Test
        void activateFromNonIdle() {
            mapper.open(1);
            mapper.activate(1);

            TransitionResult r = mapper.activate(1); // OPEN → OPEN is invalid
            assertFalse(r.valid());
            assertEquals(StreamState.OPEN, mapper.state(1));
        }

        @Test
        void halfCloseLocalFromIdle() {
            mapper.open(1);
            TransitionResult r = mapper.halfCloseLocal(1); // IDLE → HALF_CLOSED_LOCAL invalid
            assertFalse(r.valid());
        }

        @Test
        void halfCloseRemoteFromIdle() {
            mapper.open(1);
            TransitionResult r = mapper.halfCloseRemote(1); // IDLE → HALF_CLOSED_REMOTE invalid
            assertFalse(r.valid());
        }

        @Test
        void doubleHalfCloseLocal() {
            mapper.open(1);
            mapper.activate(1);
            mapper.halfCloseLocal(1);

            TransitionResult r = mapper.halfCloseLocal(1); // HALF_CLOSED_LOCAL → ? invalid
            assertFalse(r.valid());
        }

        @Test
        void doubleHalfCloseRemote() {
            mapper.open(1);
            mapper.activate(1);
            mapper.halfCloseRemote(1);

            TransitionResult r = mapper.halfCloseRemote(1); // HALF_CLOSED_REMOTE → ? invalid
            assertFalse(r.valid());
        }

        @Test
        void transitionOnUnknownStream() {
            TransitionResult r = mapper.activate(999);
            assertFalse(r.valid());
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // activateHalfClosed: IDLE -> HALF_CLOSED_REMOTE (GET with END_STREAM)
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class ActivateHalfClosed {

        @Test
        void idleToHalfClosedRemote_valid() {
            int streamId = 1;
            mapper.open(streamId);

            TransitionResult r = mapper.activateHalfClosed(streamId);
            assertTrue(r.valid());
            assertEquals(StreamState.IDLE, r.previousState());
            assertEquals(StreamState.HALF_CLOSED_REMOTE, r.newState());
            assertEquals(StreamState.HALF_CLOSED_REMOTE, mapper.state(streamId));

            // Can still send data (local side not closed)
            assertTrue(mapper.canSendData(streamId));
            // Cannot receive data (remote side is closed)
            assertFalse(mapper.canReceiveData(streamId));
        }

        @Test
        void activateHalfClosed_fromOpen_invalid() {
            mapper.open(1);
            mapper.activate(1);

            TransitionResult r = mapper.activateHalfClosed(1);
            assertFalse(r.valid(), "activateHalfClosed from OPEN must be invalid");
            assertEquals(StreamState.OPEN, mapper.state(1));
        }

        @Test
        void activateHalfClosed_thenHalfCloseLocal_closesFully() {
            mapper.open(1);
            mapper.activateHalfClosed(1);

            TransitionResult r = mapper.halfCloseLocal(1);
            assertTrue(r.valid());
            assertEquals(StreamState.CLOSED, r.newState());
        }

        @Test
        void removeReturnsRemovedState() {
            mapper.open(1);
            mapper.activate(1);
            mapper.halfCloseLocal(1);

            StreamState removed = mapper.remove(1);
            assertEquals(StreamState.HALF_CLOSED_LOCAL, removed,
                    "remove() must return the state at the time of removal");
            assertNull(mapper.state(1));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Reset transitions (RST_STREAM / QUIC RESET_STREAM / TCP close)
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class ResetTransitions {

        @Test
        void resetFromOpen() {
            mapper.open(1);
            mapper.activate(1);

            TransitionResult r = mapper.reset(1);
            assertTrue(r.valid());
            assertEquals(StreamState.RESET, r.newState());
        }

        @Test
        void resetFromHalfClosedLocal() {
            mapper.open(1);
            mapper.activate(1);
            mapper.halfCloseLocal(1);

            TransitionResult r = mapper.reset(1);
            assertTrue(r.valid());
            assertEquals(StreamState.RESET, r.newState());
        }

        @Test
        void resetFromHalfClosedRemote() {
            mapper.open(1);
            mapper.activate(1);
            mapper.halfCloseRemote(1);

            TransitionResult r = mapper.reset(1);
            assertTrue(r.valid());
            assertEquals(StreamState.RESET, r.newState());
        }

        @Test
        void resetFromIdle() {
            mapper.open(1);
            TransitionResult r = mapper.reset(1);
            assertTrue(r.valid());
            assertEquals(StreamState.RESET, r.newState());
        }

        @Test
        void doubleResetIsInvalid() {
            mapper.open(1);
            mapper.activate(1);
            mapper.reset(1);

            TransitionResult r = mapper.reset(1);
            assertFalse(r.valid());
        }

        @Test
        void resetFromClosedIsInvalid() {
            mapper.open(1);
            mapper.activate(1);
            mapper.halfCloseLocal(1);
            mapper.halfCloseRemote(1);

            TransitionResult r = mapper.reset(1);
            assertFalse(r.valid());
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Data send/receive guards
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class DataGuards {

        @Test
        void canReceiveDataInOpenState() {
            mapper.open(1);
            mapper.activate(1);
            assertTrue(mapper.canReceiveData(1));
        }

        @Test
        void canReceiveDataInHalfClosedLocal() {
            mapper.open(1);
            mapper.activate(1);
            mapper.halfCloseLocal(1);
            assertTrue(mapper.canReceiveData(1));
        }

        @Test
        void cannotReceiveDataInHalfClosedRemote() {
            mapper.open(1);
            mapper.activate(1);
            mapper.halfCloseRemote(1);
            assertFalse(mapper.canReceiveData(1));
        }

        @Test
        void canSendDataInOpenState() {
            mapper.open(1);
            mapper.activate(1);
            assertTrue(mapper.canSendData(1));
        }

        @Test
        void canSendDataInHalfClosedRemote() {
            mapper.open(1);
            mapper.activate(1);
            mapper.halfCloseRemote(1);
            assertTrue(mapper.canSendData(1));
        }

        @Test
        void cannotSendDataInHalfClosedLocal() {
            mapper.open(1);
            mapper.activate(1);
            mapper.halfCloseLocal(1);
            assertFalse(mapper.canSendData(1));
        }

        @Test
        void cannotReceiveDataOnUnknownStream() {
            assertFalse(mapper.canReceiveData(999));
        }

        @Test
        void cannotSendDataOnUnknownStream() {
            assertFalse(mapper.canSendData(999));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Cleanup
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class Cleanup {

        @Test
        void removeReturnsLastState() {
            mapper.open(1);
            mapper.activate(1);
            mapper.halfCloseLocal(1);

            StreamState removed = mapper.remove(1);
            assertEquals(StreamState.HALF_CLOSED_LOCAL, removed);
            assertNull(mapper.state(1));
        }

        @Test
        void removeUnknownReturnsNull() {
            assertNull(mapper.remove(999));
        }

        @Test
        void clearRemovesAll() {
            mapper.open(1);
            mapper.open(3);
            mapper.open(5);
            assertEquals(3, mapper.size());

            mapper.clear();
            assertEquals(0, mapper.size());
            assertNull(mapper.state(1));
        }

        @Test
        void sizeTracking() {
            assertEquals(0, mapper.size());
            mapper.open(1);
            assertEquals(1, mapper.size());
            mapper.open(3);
            assertEquals(2, mapper.size());
            mapper.remove(1);
            assertEquals(1, mapper.size());
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Multiple concurrent streams (H2 and H3 multiplexing)
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class MultipleStreams {

        @Test
        void independentStreamLifecycles() {
            // Stream 1: normal close
            mapper.open(1);
            mapper.activate(1);
            mapper.halfCloseLocal(1);

            // Stream 3: still open
            mapper.open(3);
            mapper.activate(3);

            // Stream 5: reset
            mapper.open(5);
            mapper.activate(5);
            mapper.reset(5);

            assertEquals(StreamState.HALF_CLOSED_LOCAL, mapper.state(1));
            assertEquals(StreamState.OPEN, mapper.state(3));
            assertEquals(StreamState.RESET, mapper.state(5));

            // Complete stream 1
            mapper.halfCloseRemote(1);
            assertEquals(StreamState.CLOSED, mapper.state(1));

            // Stream 3 unaffected
            assertEquals(StreamState.OPEN, mapper.state(3));
        }

        @Test
        void h1SerialRequests() {
            // H1 uses stream ID 0 for serial requests
            // Request 1
            mapper.open(0);
            mapper.activate(0);
            mapper.halfCloseLocal(0);
            mapper.halfCloseRemote(0);
            assertEquals(StreamState.CLOSED, mapper.state(0));

            // Cleanup for reuse
            mapper.remove(0);

            // Request 2 on the same connection
            mapper.open(0);
            mapper.activate(0);
            assertEquals(StreamState.OPEN, mapper.state(0));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Thread safety
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class ThreadSafety {

        @Test
        void concurrentTransitions() throws InterruptedException {
            int numStreams = 1000;
            ExecutorService executor = Executors.newFixedThreadPool(8);
            CountDownLatch latch = new CountDownLatch(numStreams);
            AtomicInteger closedCount = new AtomicInteger(0);

            // Pre-open all streams
            for (int i = 0; i < numStreams; i++) {
                mapper.open(i);
                mapper.activate(i);
            }

            // Concurrently transition all streams through their lifecycle
            for (int i = 0; i < numStreams; i++) {
                final int streamId = i;
                executor.submit(() -> {
                    try {
                        mapper.halfCloseLocal(streamId);
                        mapper.halfCloseRemote(streamId);
                        if (mapper.state(streamId) == StreamState.CLOSED) {
                            closedCount.incrementAndGet();
                        }
                    } finally {
                        latch.countDown();
                    }
                });
            }

            latch.await();
            executor.shutdown();

            assertEquals(numStreams, closedCount.get(),
                    "All streams should reach CLOSED state");
        }
    }
}
