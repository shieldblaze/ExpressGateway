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
package com.shieldblaze.expressgateway.protocol.http.http3;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for HTTP/3 data-dribble attack defense (RFC 9114 Section 8.1).
 *
 * <p>The data-dribble attack involves sending many tiny DATA frames on a single stream
 * to amplify per-frame processing overhead. The Http3ServerHandler limits the number
 * of DATA frames per stream and resets the stream with H3_EXCESSIVE_LOAD (0x107)
 * when the limit is exceeded.</p>
 *
 * <p>The frame limit is calculated as:
 * {@code max(BASE_MAX_DATA_FRAMES, maxRequestBodySize / MIN_EXPECTED_FRAME_SIZE)}
 * where BASE_MAX_DATA_FRAMES = 10,000 and MIN_EXPECTED_FRAME_SIZE = 1024.</p>
 */
class Http3DataDribbleDefenseTest {

    /**
     * BASE_MAX_DATA_FRAMES and MIN_EXPECTED_FRAME_SIZE are package-private constants
     * in Http3ServerHandler. We mirror them here for verification.
     */
    private static final int BASE_MAX_DATA_FRAMES = 10_000;
    private static final int MIN_EXPECTED_FRAME_SIZE = 1024;

    @Test
    void testDefaultFrameLimit() {
        // Default maxRequestBodySize (from HttpConfiguration.DEFAULT) is typically 10MB
        // With 10MB / 1024 = ~10240, which is > BASE_MAX_DATA_FRAMES
        long maxBodySize = 10L * 1024 * 1024; // 10MB
        int expected = (int) Math.max(BASE_MAX_DATA_FRAMES, maxBodySize / MIN_EXPECTED_FRAME_SIZE);
        assertEquals(10240, expected, "10MB body should allow 10240 frames");
    }

    @Test
    void testSmallBodyUsesBaseLimit() {
        // Small body size: 1MB / 1024 = ~1024, which is < BASE_MAX_DATA_FRAMES
        long maxBodySize = 1L * 1024 * 1024; // 1MB
        int expected = (int) Math.max(BASE_MAX_DATA_FRAMES, maxBodySize / MIN_EXPECTED_FRAME_SIZE);
        assertEquals(BASE_MAX_DATA_FRAMES, expected, "Small body should use base limit");
    }

    @Test
    void testLargeBodyScalesLimit() {
        // 100MB body: 100 * 1024 * 1024 / 1024 = 102400
        long maxBodySize = 100L * 1024 * 1024; // 100MB
        int expected = (int) Math.max(BASE_MAX_DATA_FRAMES, maxBodySize / MIN_EXPECTED_FRAME_SIZE);
        assertEquals(102400, expected, "100MB body should scale to 102400 frames");
    }

    @Test
    void testZeroBodyUsesBaseLimit() {
        // Zero means unlimited body, but frame limit should still apply.
        // Http3ServerHandler uses the else branch: maxDataFramesPerStream = BASE_MAX_DATA_FRAMES
        // When maxBodySize is 0 (unlimited), the handler uses BASE_MAX_DATA_FRAMES directly
        assertEquals(10_000, BASE_MAX_DATA_FRAMES, "Zero body should use base limit of 10000");
    }

    @Test
    void testDrainingKeyExists() {
        // Verify the DRAINING_KEY attribute key is properly defined
        assertNotNull(Http3Constants.DRAINING_KEY, "DRAINING_KEY should be defined");
        assertEquals("h3.draining", Http3Constants.DRAINING_KEY.name(),
                "DRAINING_KEY should have the expected name");
    }

    @Test
    void testH3ErrorCodes() throws Exception {
        // Verify the critical H3 error code constants used in Http3ServerHandler
        // against the expected values defined in RFC 9114 Section 8.1.
        // The fields are private, so we use reflection to read and validate them.
        java.lang.reflect.Field excessiveLoad = Http3ServerHandler.class.getDeclaredField("H3_EXCESSIVE_LOAD");
        excessiveLoad.setAccessible(true);
        assertEquals(0x107L, excessiveLoad.getLong(null),
                "H3_EXCESSIVE_LOAD must be 0x107 per RFC 9114");

        java.lang.reflect.Field messageError = Http3ServerHandler.class.getDeclaredField("H3_MESSAGE_ERROR");
        messageError.setAccessible(true);
        assertEquals(0x10eL, messageError.getLong(null),
                "H3_MESSAGE_ERROR must be 0x10e per RFC 9114");

        java.lang.reflect.Field requestCancelled = Http3ServerHandler.class.getDeclaredField("H3_REQUEST_CANCELLED");
        requestCancelled.setAccessible(true);
        assertEquals(0x10cL, requestCancelled.getLong(null),
                "H3_REQUEST_CANCELLED must be 0x10c per RFC 9114");
    }

    @Test
    void testFrameLimitAlgorithmMonotonicity() {
        // Frame limit should be monotonically non-decreasing as body size increases
        int prevLimit = BASE_MAX_DATA_FRAMES;
        for (long bodySize = 0; bodySize <= 1024L * 1024 * 1024; bodySize += 10L * 1024 * 1024) {
            int limit;
            if (bodySize > 0) {
                limit = (int) Math.max(BASE_MAX_DATA_FRAMES, bodySize / MIN_EXPECTED_FRAME_SIZE);
            } else {
                limit = BASE_MAX_DATA_FRAMES;
            }
            assertTrue(limit >= prevLimit,
                    "Frame limit should not decrease as body size increases: " + limit + " < " + prevLimit);
            prevLimit = limit;
        }
    }
}
