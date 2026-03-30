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
package com.shieldblaze.expressgateway.configuration.distributed;

import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ConfigValidatorTest {

    @Test
    void testDefaultConfigurationPasses() {
        List<String> errors = ConfigValidator.validate(ConfigurationContext.DEFAULT);
        assertTrue(errors.isEmpty(), "Default configuration should pass validation, but got: " + errors);
    }

    @Test
    void testNullContextThrows() {
        assertThrows(NullPointerException.class, () -> ConfigValidator.validate(null));
    }

    @Test
    void testMissingRequiredFieldsDetected() {
        // Create a context with null bufferConfiguration
        ConfigurationContext context = new ConfigurationContext(
                null, // bufferConfiguration = null
                ConfigurationContext.DEFAULT.eventLoopConfiguration(),
                ConfigurationContext.DEFAULT.eventStreamConfiguration(),
                ConfigurationContext.DEFAULT.healthCheckConfiguration(),
                ConfigurationContext.DEFAULT.httpConfiguration(),
                ConfigurationContext.DEFAULT.http3Configuration(),
                ConfigurationContext.DEFAULT.quicConfiguration(),
                ConfigurationContext.DEFAULT.tlsClientConfiguration(),
                ConfigurationContext.DEFAULT.tlsServerConfiguration(),
                ConfigurationContext.DEFAULT.transportConfiguration()
        );

        List<String> errors = ConfigValidator.validate(context);
        assertTrue(errors.stream().anyMatch(e -> e.contains("bufferConfiguration must not be null")),
                "Should report missing bufferConfiguration, got: " + errors);
    }

    @Test
    void testMultipleMissingFieldsReported() {
        ConfigurationContext context = new ConfigurationContext(
                null, null, null, null, null, null, null, null, null, null
        );

        List<String> errors = ConfigValidator.validate(context);
        // Should report all 10 required fields as missing
        assertTrue(errors.size() >= 10,
                "Should report at least 10 missing fields, got " + errors.size() + ": " + errors);
    }

    @Test
    void testCrossFieldConsistency_Http3WithoutQuic() {
        // HTTP/3 without QUIC should flag a cross-field error
        ConfigurationContext context = new ConfigurationContext(
                ConfigurationContext.DEFAULT.bufferConfiguration(),
                ConfigurationContext.DEFAULT.eventLoopConfiguration(),
                ConfigurationContext.DEFAULT.eventStreamConfiguration(),
                ConfigurationContext.DEFAULT.healthCheckConfiguration(),
                ConfigurationContext.DEFAULT.httpConfiguration(),
                ConfigurationContext.DEFAULT.http3Configuration(),
                null, // quicConfiguration = null
                ConfigurationContext.DEFAULT.tlsClientConfiguration(),
                ConfigurationContext.DEFAULT.tlsServerConfiguration(),
                ConfigurationContext.DEFAULT.transportConfiguration()
        );

        List<String> errors = ConfigValidator.validate(context);
        assertTrue(errors.stream().anyMatch(e -> e.contains("HTTP/3 requires QUIC")),
                "Should flag HTTP/3 without QUIC, got: " + errors);
    }

    @Test
    void testCrossFieldConsistency_QuicWithoutTls() {
        // QUIC without TLS server should flag a cross-field error
        ConfigurationContext context = new ConfigurationContext(
                ConfigurationContext.DEFAULT.bufferConfiguration(),
                ConfigurationContext.DEFAULT.eventLoopConfiguration(),
                ConfigurationContext.DEFAULT.eventStreamConfiguration(),
                ConfigurationContext.DEFAULT.healthCheckConfiguration(),
                ConfigurationContext.DEFAULT.httpConfiguration(),
                ConfigurationContext.DEFAULT.http3Configuration(),
                ConfigurationContext.DEFAULT.quicConfiguration(),
                ConfigurationContext.DEFAULT.tlsClientConfiguration(),
                null, // tlsServerConfiguration = null
                ConfigurationContext.DEFAULT.transportConfiguration()
        );

        List<String> errors = ConfigValidator.validate(context);
        assertTrue(errors.stream().anyMatch(e -> e.contains("QUIC mandates TLS")),
                "Should flag QUIC without TLS, got: " + errors);
    }

    @Test
    void testJsonRoundTripWithValidConfig() {
        // Default config should pass JSON round-trip
        List<String> errors = ConfigValidator.validate(ConfigurationContext.DEFAULT);
        assertTrue(errors.stream().noneMatch(e -> e.contains("JSON round-trip")),
                "Default config JSON round-trip should pass, got: " + errors);
    }

    @Test
    void testResultIsUnmodifiable() {
        List<String> errors = ConfigValidator.validate(ConfigurationContext.DEFAULT);
        assertThrows(UnsupportedOperationException.class, () -> errors.add("test"));
    }
}
