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
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

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
        config.setMaxInitialLineLength(4096);
        config.setMaxHeaderSize(8192);
        config.setMaxChunkSize(8192);
        config.setCompressionThreshold(1024);
        config.setDeflateCompressionLevel(6);
        config.setBrotliCompressionLevel(4);
        config.setMaxConcurrentStreams(100);
        config.setBackendResponseTimeoutSeconds(60);
        config.setMaxRequestBodySize(10L * 1024 * 1024);
        config.setInitialWindowSize(1048576);
        config.setH2ConnectionWindowSize(1048576);
        config.setMaxConnectionBodySize(256L * 1024 * 1024);
        config.setMaxHeaderListSize(8192);
        config.setRequestHeaderTimeoutSeconds(30);
        config.setMaxH1ConnectionsPerNode(32);
        config.setMaxH2ConnectionsPerNode(4);
        config.setPoolIdleTimeoutSeconds(60);
        config.setGracefulShutdownDrainMs(5000);
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
        config.setMaxConcurrentStreams(250);
        config.validate();

        assertEquals(250, config.maxConcurrentStreams(),
                "maxConcurrentStreams must return the configured value");
    }

    @Test
    void testMaxConcurrentStreamsValidationRejectsZero() {
        HttpConfiguration config = validConfig();
        config.setMaxConcurrentStreams(0);

        assertThrows(IllegalArgumentException.class, config::validate,
                "maxConcurrentStreams of 0 must be rejected by validation");
    }

    @Test
    void testMaxConcurrentStreamsValidationRejectsNegative() {
        HttpConfiguration config = validConfig();
        config.setMaxConcurrentStreams(-1);

        assertThrows(IllegalArgumentException.class, config::validate,
                "Negative maxConcurrentStreams must be rejected by validation");
    }

    @Test
    void testMaxConcurrentStreamsValidationAcceptsOne() {
        HttpConfiguration config = validConfig();
        config.setMaxConcurrentStreams(1);

        assertDoesNotThrow(config::validate,
                "maxConcurrentStreams of 1 must be accepted");
        assertEquals(1, config.maxConcurrentStreams());
    }

    @Test
    void testMaxConcurrentStreamsLargeValue() {
        HttpConfiguration config = validConfig();
        config.setMaxConcurrentStreams(Integer.MAX_VALUE);

        assertDoesNotThrow(config::validate,
                "Large maxConcurrentStreams value must be accepted");
        assertEquals(Integer.MAX_VALUE, config.maxConcurrentStreams());
    }

    @Test
    void testDefaultConfigurationIsValidated() {
        // The DEFAULT static instance must already be validated
        assertTrue(HttpConfiguration.DEFAULT.validated(),
                "DEFAULT configuration must be pre-validated");
    }

    @Test
    void testConfigurationNotValidatedBeforeValidateCall() {
        HttpConfiguration config = validConfig();

        // Before validate() is called, the config must report as not validated
        assertFalse(config.validated(),
                "Configuration must not be validated before validate() is called");

        config.validate();

        assertTrue(config.validated(),
                "Configuration must be validated after validate() is called");
        assertEquals(100, config.maxConcurrentStreams());
    }
}
