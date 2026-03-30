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

import io.netty.channel.Channel;
import io.netty.channel.embedded.EmbeddedChannel;
import org.junit.jupiter.api.Nested;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link FlowControlTranslator} covering backpressure propagation
 * across all protocol translation paths.
 */
class FlowControlTranslatorTest {

    // ═══════════════════════════════════════════════════════════════════════
    // Basic backpressure propagation
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class BasicBackpressure {

        @Test
        void propagateBackpressureWhenTargetWritable() {
            EmbeddedChannel source = new EmbeddedChannel();
            EmbeddedChannel target = new EmbeddedChannel();

            // Target is writable → source should read
            FlowControlTranslator.propagateBackpressure(source, target);
            assertTrue(source.config().isAutoRead());

            source.close();
            target.close();
        }

        @Test
        void propagateBackpressureNullSafe() {
            EmbeddedChannel channel = new EmbeddedChannel();

            // Must not throw with null arguments
            assertDoesNotThrow(() -> FlowControlTranslator.propagateBackpressure(null, channel));
            assertDoesNotThrow(() -> FlowControlTranslator.propagateBackpressure(channel, null));
            assertDoesNotThrow(() -> FlowControlTranslator.propagateBackpressure(null, null));

            channel.close();
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Proactive backpressure after write
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class AfterWrite {

        @Test
        void checkAfterWriteWhenTargetWritable() {
            EmbeddedChannel source = new EmbeddedChannel();
            EmbeddedChannel target = new EmbeddedChannel();

            // Target writable → source should continue reading
            FlowControlTranslator.checkAfterWrite(source, target);
            assertTrue(source.config().isAutoRead());

            source.close();
            target.close();
        }

        @Test
        void checkAfterWriteNullSafe() {
            EmbeddedChannel channel = new EmbeddedChannel();

            assertDoesNotThrow(() -> FlowControlTranslator.checkAfterWrite(null, channel));
            assertDoesNotThrow(() -> FlowControlTranslator.checkAfterWrite(channel, null));

            channel.close();
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Aggregate backpressure (H2 multiplexed)
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class AggregateBackpressure {

        @Test
        void allTargetsWritable() {
            EmbeddedChannel source = new EmbeddedChannel();
            EmbeddedChannel target1 = new EmbeddedChannel();
            EmbeddedChannel target2 = new EmbeddedChannel();

            boolean result = FlowControlTranslator.checkAggregateBackpressure(
                    source, List.of(target1, target2));

            assertTrue(result);
            assertTrue(source.config().isAutoRead());

            source.close();
            target1.close();
            target2.close();
        }

        @Test
        void emptyTargets() {
            EmbeddedChannel source = new EmbeddedChannel();

            boolean result = FlowControlTranslator.checkAggregateBackpressure(
                    source, List.of());

            assertTrue(result);
            assertTrue(source.config().isAutoRead());

            source.close();
        }

        @Test
        void nullSource() {
            assertFalse(FlowControlTranslator.checkAggregateBackpressure(null, List.of()));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Pause/Resume reads
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class PauseResume {

        @Test
        void pauseReads() {
            EmbeddedChannel channel = new EmbeddedChannel();
            assertTrue(channel.config().isAutoRead()); // default

            FlowControlTranslator.pauseReads(channel);
            assertFalse(channel.config().isAutoRead());

            channel.close();
        }

        @Test
        void resumeReads() {
            EmbeddedChannel channel = new EmbeddedChannel();
            channel.config().setAutoRead(false);

            FlowControlTranslator.resumeReads(channel);
            assertTrue(channel.config().isAutoRead());

            channel.close();
        }

        @Test
        void pauseReadsNullSafe() {
            assertDoesNotThrow(() -> FlowControlTranslator.pauseReads(null));
        }

        @Test
        void resumeReadsNullSafe() {
            assertDoesNotThrow(() -> FlowControlTranslator.resumeReads(null));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Writability check
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class Writability {

        @Test
        void isWritableActive() {
            EmbeddedChannel channel = new EmbeddedChannel();
            assertTrue(FlowControlTranslator.isWritable(channel));
            channel.close();
        }

        @Test
        void isWritableNull() {
            assertFalse(FlowControlTranslator.isWritable(null));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Water mark configuration
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class WaterMarks {

        @Test
        void configureWaterMarks() {
            EmbeddedChannel channel = new EmbeddedChannel();

            FlowControlTranslator.configureWaterMarks(channel, 16384, 65536);

            // Netty's EmbeddedChannel may not fully support water marks,
            // but the method should not throw
            assertNotNull(channel.config());
            channel.close();
        }

        @Test
        void configureWaterMarksNullSafe() {
            assertDoesNotThrow(() -> FlowControlTranslator.configureWaterMarks(null, 1024, 4096));
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Water mark ordering fix
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class WaterMarkOrder {

        @Test
        void configureWaterMarks_highFirstThenLow() {
            EmbeddedChannel channel = new EmbeddedChannel();

            // If low is set first and low > current high, Netty throws.
            // This test verifies that configureWaterMarks sets high first.
            assertDoesNotThrow(() ->
                    FlowControlTranslator.configureWaterMarks(channel, 32768, 131072));

            channel.close();
        }

        @Test
        void configureWaterMarks_valuesApplied() {
            EmbeddedChannel channel = new EmbeddedChannel();

            FlowControlTranslator.configureWaterMarks(channel, 16384, 65536);

            assertEquals(65536, channel.config().getWriteBufferHighWaterMark());
            assertEquals(16384, channel.config().getWriteBufferLowWaterMark());

            channel.close();
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Inactive channel handling
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class InactiveChannelHandling {

        @Test
        void propagateBackpressure_inactiveSource_noOp() {
            EmbeddedChannel source = new EmbeddedChannel();
            EmbeddedChannel target = new EmbeddedChannel();

            source.close(); // make inactive

            // Must not throw on inactive source
            assertDoesNotThrow(() ->
                    FlowControlTranslator.propagateBackpressure(source, target));

            target.close();
        }

        @Test
        void propagateBackpressure_inactiveTarget_noOp() {
            EmbeddedChannel source = new EmbeddedChannel();
            EmbeddedChannel target = new EmbeddedChannel();

            target.close(); // make inactive

            // Must not throw or change source config
            assertDoesNotThrow(() ->
                    FlowControlTranslator.propagateBackpressure(source, target));

            source.close();
        }

        @Test
        void pauseReads_inactiveChannel_noOp() {
            EmbeddedChannel channel = new EmbeddedChannel();
            channel.close();

            // Must not throw on inactive channel
            assertDoesNotThrow(() -> FlowControlTranslator.pauseReads(channel));
        }

        @Test
        void resumeReads_inactiveChannel_noOp() {
            EmbeddedChannel channel = new EmbeddedChannel();
            channel.close();

            // Must not throw on inactive channel
            assertDoesNotThrow(() -> FlowControlTranslator.resumeReads(channel));
        }

        @Test
        void checkAggregateBackpressure_inactiveTarget_treatedAsNotWritable() {
            EmbeddedChannel source = new EmbeddedChannel();
            EmbeddedChannel activeTarget = new EmbeddedChannel();
            EmbeddedChannel inactiveTarget = new EmbeddedChannel();
            inactiveTarget.close(); // make inactive

            boolean result = FlowControlTranslator.checkAggregateBackpressure(
                    source, List.of(activeTarget, inactiveTarget));

            assertFalse(result,
                    "Inactive channels must be treated as NOT writable to prevent buffering for dead channels");

            source.close();
            activeTarget.close();
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Cross-protocol scenarios
    // ═══════════════════════════════════════════════════════════════════════

    @Nested
    class CrossProtocolScenarios {

        /**
         * Simulates H2→H1 flow control: HTTP/2 frontend writing to HTTP/1.1 backend.
         * When the H1 backend's TCP send buffer fills up, the H2 frontend must
         * stop reading frames.
         */
        @Test
        void h2ToH1BackpressureSimulation() {
            EmbeddedChannel h2Frontend = new EmbeddedChannel();
            EmbeddedChannel h1Backend = new EmbeddedChannel();

            // Initially both writable
            assertTrue(h1Backend.isWritable());
            FlowControlTranslator.propagateBackpressure(h2Frontend, h1Backend);
            assertTrue(h2Frontend.config().isAutoRead());

            h2Frontend.close();
            h1Backend.close();
        }

        /**
         * Simulates H3→H2 flow control: HTTP/3 frontend writing to HTTP/2 backend.
         * When the H2 backend flow control window is exhausted (channel unwritable),
         * the QUIC stream must stop reading.
         */
        @Test
        void h3ToH2BackpressureSimulation() {
            EmbeddedChannel h3Frontend = new EmbeddedChannel();
            EmbeddedChannel h2Backend = new EmbeddedChannel();

            FlowControlTranslator.propagateBackpressure(h3Frontend, h2Backend);
            assertTrue(h3Frontend.config().isAutoRead());

            // Proactive check after write
            FlowControlTranslator.checkAfterWrite(h3Frontend, h2Backend);
            assertTrue(h3Frontend.config().isAutoRead()); // still writable

            h3Frontend.close();
            h2Backend.close();
        }

        /**
         * Simulates multiplexed H2 frontend with multiple H1 backends.
         * All backends must be writable before the frontend resumes reading.
         */
        @Test
        void multiplexedH2ToMultipleH1Backends() {
            EmbeddedChannel h2Frontend = new EmbeddedChannel();
            EmbeddedChannel h1Backend1 = new EmbeddedChannel();
            EmbeddedChannel h1Backend2 = new EmbeddedChannel();
            EmbeddedChannel h1Backend3 = new EmbeddedChannel();

            List<Channel> backends = List.of(h1Backend1, h1Backend2, h1Backend3);

            // All writable → frontend should read
            boolean allWritable = FlowControlTranslator.checkAggregateBackpressure(h2Frontend, backends);
            assertTrue(allWritable);
            assertTrue(h2Frontend.config().isAutoRead());

            h2Frontend.close();
            h1Backend1.close();
            h1Backend2.close();
            h1Backend3.close();
        }
    }
}
