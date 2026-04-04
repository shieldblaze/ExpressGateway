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
package com.shieldblaze.expressgateway.configuration.http;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

/**
 * Tests for {@link HttpConfiguration}, focusing on the maxConcurrentStreams
 * setting (RFC 9113 Section 6.5.2 SETTINGS_MAX_CONCURRENT_STREAMS) and
 * general configuration validation.
 */
class HttpConfigurationTest {

    /**
     * Creates a valid HttpConfiguration with all required fields set to defaults.
     */
    private static HttpConfiguration validConfig() {
        HttpConfiguration config = new HttpConfiguration();
        config.maxInitialLineLength(4096);
        config.maxHeaderSize(8192);
        config.maxChunkSize(8192);
        config.compressionThreshold(1024);
        config.deflateCompressionLevel(6);
        config.brotliCompressionLevel(4);
        config.maxConcurrentStreams(100);
        config.backendResponseTimeoutSeconds(60);
        config.maxRequestBodySize(10L * 1024 * 1024);
        config.initialWindowSize(1048576);
        config.h2ConnectionWindowSize(1048576);
        config.maxConnectionBodySize(256L * 1024 * 1024);
        config.maxHeaderListSize(8192);
        config.requestHeaderTimeoutSeconds(30);
        config.maxH1ConnectionsPerNode(32);
        config.maxH2ConnectionsPerNode(4);
        config.poolIdleTimeoutSeconds(60);
        config.gracefulShutdownDrainMs(5000);
        return config;
    }

    @Test
    void testDefaultMaxConcurrentStreams() {
        // The DEFAULT instance has maxConcurrentStreams = 100
        assertEquals(100, HttpConfiguration.DEFAULT.maxConcurrentStreams(),
                "Default maxConcurrentStreams must be 100 per RFC 9113 recommendation");
    }

    @Test
    void testCustomMaxConcurrentStreams() {
        HttpConfiguration config = validConfig();
        config.maxConcurrentStreams(250);
        config.validate();

        assertEquals(250, config.maxConcurrentStreams(),
                "maxConcurrentStreams must return the configured value");
    }

    @Test
    void testMaxConcurrentStreamsValidationRejectsZero() {
        HttpConfiguration config = validConfig();
        config.maxConcurrentStreams(0);

        assertThrows(IllegalArgumentException.class, config::validate,
                "maxConcurrentStreams of 0 must be rejected by validation");
    }

    @Test
    void testMaxConcurrentStreamsValidationRejectsNegative() {
        HttpConfiguration config = validConfig();
        config.maxConcurrentStreams(-1);

        assertThrows(IllegalArgumentException.class, config::validate,
                "Negative maxConcurrentStreams must be rejected by validation");
    }

    @Test
    void testMaxConcurrentStreamsValidationAcceptsOne() {
        HttpConfiguration config = validConfig();
        config.maxConcurrentStreams(1);

        assertDoesNotThrow(config::validate,
                "maxConcurrentStreams of 1 must be accepted");
        assertEquals(1, config.maxConcurrentStreams());
    }

    @Test
    void testMaxConcurrentStreamsLargeValue() {
        HttpConfiguration config = validConfig();
        config.maxConcurrentStreams(Integer.MAX_VALUE);

        assertDoesNotThrow(config::validate,
                "Large maxConcurrentStreams value must be accepted");
        assertEquals(Integer.MAX_VALUE, config.maxConcurrentStreams());
    }

}
