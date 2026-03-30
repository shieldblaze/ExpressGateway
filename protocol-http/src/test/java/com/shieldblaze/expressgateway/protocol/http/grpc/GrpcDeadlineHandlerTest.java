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
package com.shieldblaze.expressgateway.protocol.http.grpc;

import org.junit.jupiter.api.Test;

import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.ScheduledThreadPoolExecutor;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class GrpcDeadlineHandlerTest {

    // ─── Timeout Parsing ───

    @Test
    void testParseHours() {
        assertEquals(3_600_000_000_000L, GrpcDeadlineHandler.parseTimeoutNanos("1H"));
        assertEquals(7_200_000_000_000L, GrpcDeadlineHandler.parseTimeoutNanos("2H"));
    }

    @Test
    void testParseMinutes() {
        assertEquals(60_000_000_000L, GrpcDeadlineHandler.parseTimeoutNanos("1M"));
        assertEquals(300_000_000_000L, GrpcDeadlineHandler.parseTimeoutNanos("5M"));
    }

    @Test
    void testParseSeconds() {
        assertEquals(1_000_000_000L, GrpcDeadlineHandler.parseTimeoutNanos("1S"));
        assertEquals(5_000_000_000L, GrpcDeadlineHandler.parseTimeoutNanos("5S"));
    }

    @Test
    void testParseMilliseconds() {
        assertEquals(100_000_000L, GrpcDeadlineHandler.parseTimeoutNanos("100m"));
        assertEquals(1_000_000L, GrpcDeadlineHandler.parseTimeoutNanos("1m"));
    }

    @Test
    void testParseMicroseconds() {
        assertEquals(1_000L, GrpcDeadlineHandler.parseTimeoutNanos("1u"));
        assertEquals(500_000L, GrpcDeadlineHandler.parseTimeoutNanos("500u"));
    }

    @Test
    void testParseNanoseconds() {
        assertEquals(1L, GrpcDeadlineHandler.parseTimeoutNanos("1n"));
        assertEquals(999_999_999L, GrpcDeadlineHandler.parseTimeoutNanos("999999999n"));
    }

    @Test
    void testParseZeroValue() {
        assertEquals(0L, GrpcDeadlineHandler.parseTimeoutNanos("0S"));
        assertEquals(0L, GrpcDeadlineHandler.parseTimeoutNanos("0n"));
    }

    @Test
    void testParseNull() {
        assertEquals(-1L, GrpcDeadlineHandler.parseTimeoutNanos(null));
    }

    @Test
    void testParseEmpty() {
        assertEquals(-1L, GrpcDeadlineHandler.parseTimeoutNanos(""));
    }

    @Test
    void testParseTooShort() {
        // Must be at least 2 chars (digit + unit)
        assertEquals(-1L, GrpcDeadlineHandler.parseTimeoutNanos("S"));
        assertEquals(-1L, GrpcDeadlineHandler.parseTimeoutNanos("1"));
    }

    @Test
    void testParseInvalidUnit() {
        assertEquals(-1L, GrpcDeadlineHandler.parseTimeoutNanos("5X"));
        assertEquals(-1L, GrpcDeadlineHandler.parseTimeoutNanos("5s")); // lowercase s is not valid
    }

    @Test
    void testParseNonNumericValue() {
        assertEquals(-1L, GrpcDeadlineHandler.parseTimeoutNanos("abcS"));
    }

    @Test
    void testParseNegativeValue() {
        assertEquals(-1L, GrpcDeadlineHandler.parseTimeoutNanos("-5S"));
    }

    @Test
    void testParseOverflowClampsToMaxValue() {
        // Very large hour value should clamp to Long.MAX_VALUE instead of overflowing
        assertEquals(Long.MAX_VALUE, GrpcDeadlineHandler.parseTimeoutNanos("9999999999999999H"));
    }

    // ─── Deadline Cancellation ───

    @Test
    void testCancelDeadline() {
        ScheduledThreadPoolExecutor executor = new ScheduledThreadPoolExecutor(1);
        try {
            ConcurrentHashMap<Integer, ScheduledFuture<?>> deadlines = new ConcurrentHashMap<>();
            ScheduledFuture<?> future = executor.schedule(() -> {}, 1, TimeUnit.HOURS);
            deadlines.put(3, future);

            GrpcDeadlineHandler.cancelDeadline(deadlines, 3);

            assertTrue(future.isCancelled(), "Deadline should be cancelled");
            assertFalse(deadlines.containsKey(3), "Deadline should be removed from map");
        } finally {
            executor.shutdownNow();
        }
    }

    @Test
    void testCancelDeadlineNonExistentStreamIsNoOp() {
        ConcurrentHashMap<Integer, ScheduledFuture<?>> deadlines = new ConcurrentHashMap<>();
        // Should not throw
        GrpcDeadlineHandler.cancelDeadline(deadlines, 999);
        assertTrue(deadlines.isEmpty());
    }

    @Test
    void testCancelAllDeadlines() {
        ScheduledThreadPoolExecutor executor = new ScheduledThreadPoolExecutor(1);
        try {
            ConcurrentHashMap<Integer, ScheduledFuture<?>> deadlines = new ConcurrentHashMap<>();
            ScheduledFuture<?> f1 = executor.schedule(() -> {}, 1, TimeUnit.HOURS);
            ScheduledFuture<?> f2 = executor.schedule(() -> {}, 1, TimeUnit.HOURS);
            ScheduledFuture<?> f3 = executor.schedule(() -> {}, 1, TimeUnit.HOURS);
            deadlines.put(1, f1);
            deadlines.put(3, f2);
            deadlines.put(5, f3);

            GrpcDeadlineHandler.cancelAllDeadlines(deadlines);

            assertTrue(f1.isCancelled());
            assertTrue(f2.isCancelled());
            assertTrue(f3.isCancelled());
            assertTrue(deadlines.isEmpty(), "Map should be cleared");
        } finally {
            executor.shutdownNow();
        }
    }
}
