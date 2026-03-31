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
package com.shieldblaze.expressgateway.controlplane.kvstore;

import org.junit.jupiter.api.Test;

import java.util.Collections;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link StorageConfiguration} validation.
 *
 * <p>Validates that the configuration's {@link StorageConfiguration#validate()} method
 * correctly identifies invalid configurations and passes valid ones. This is critical
 * because an invalid configuration reaching production would cause the control plane
 * to fail at runtime instead of at startup.</p>
 */
class StorageConfigurationTest {

    @Test
    void testValidConfigurationPassesValidation() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("http://localhost:2379"))
                .connectTimeoutMs(5000)
                .operationTimeoutMs(10000)
                .startupHealthCheckRetries(3)
                .maxAcceptableLatencyMs(500)
                .healthCheckIntervalMs(10000)
                .startupHealthCheckTimeoutMs(30000)
                .zkSessionTimeoutMs(30000);

        assertDoesNotThrow(config::validate, "Valid configuration should not throw");
    }

    @Test
    void testMissingEndpointsFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(Collections.emptyList());

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("Endpoints"),
                "Error message should mention Endpoints: " + ex.getMessage());
    }

    @Test
    void testNullEndpointsFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(null);

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("Endpoints"),
                "Error message should mention Endpoints: " + ex.getMessage());
    }

    @Test
    void testBlankEndpointEntryFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("http://valid:2379", "  "));

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("Endpoints[1]"),
                "Error message should identify the blank endpoint by index: " + ex.getMessage());
    }

    @Test
    void testInvalidConnectTimeoutFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("http://localhost:2379"))
                .connectTimeoutMs(0);

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("ConnectTimeoutMs"),
                "Error message should mention ConnectTimeoutMs: " + ex.getMessage());
    }

    @Test
    void testNegativeConnectTimeoutFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("http://localhost:2379"))
                .connectTimeoutMs(-100);

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("ConnectTimeoutMs"),
                "Error message should mention ConnectTimeoutMs: " + ex.getMessage());
    }

    @Test
    void testInvalidOperationTimeoutFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("http://localhost:2379"))
                .operationTimeoutMs(0);

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("OperationTimeoutMs"),
                "Error message should mention OperationTimeoutMs: " + ex.getMessage());
    }

    @Test
    void testTlsEnabledWithoutCaPathFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("https://localhost:2379"))
                .tlsEnabled(true)
                .tlsCaPath(null);

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("TlsCaPath"),
                "Error message should mention TlsCaPath: " + ex.getMessage());
    }

    @Test
    void testTlsEnabledWithBlankCaPathFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("https://localhost:2379"))
                .tlsEnabled(true)
                .tlsCaPath("   ");

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("TlsCaPath"),
                "Error message should mention TlsCaPath: " + ex.getMessage());
    }

    @Test
    void testTlsDisabledWithoutCaPathPasses() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("http://localhost:2379"))
                .tlsEnabled(false)
                .tlsCaPath(null);

        assertDoesNotThrow(config::validate,
                "TLS disabled should not require TlsCaPath");
    }

    @Test
    void testTlsEnabledWithValidCaPathPasses() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("https://localhost:2379"))
                .tlsEnabled(true)
                .tlsCaPath("/path/to/ca.pem");

        assertDoesNotThrow(config::validate);
    }

    @Test
    void testZkSessionTimeoutTooLowFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("localhost:2181"))
                .zkSessionTimeoutMs(500);

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("ZkSessionTimeoutMs"),
                "Error message should mention ZkSessionTimeoutMs: " + ex.getMessage());
    }

    @Test
    void testInvalidHealthCheckIntervalFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("http://localhost:2379"))
                .healthCheckIntervalMs(0);

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("HealthCheckIntervalMs"),
                "Error message should mention HealthCheckIntervalMs: " + ex.getMessage());
    }

    @Test
    void testInvalidStartupHealthCheckTimeoutFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("http://localhost:2379"))
                .startupHealthCheckTimeoutMs(0);

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("StartupHealthCheckTimeoutMs"),
                "Error message should mention StartupHealthCheckTimeoutMs: " + ex.getMessage());
    }

    @Test
    void testInvalidStartupHealthCheckRetriesFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("http://localhost:2379"))
                .startupHealthCheckRetries(0);

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("StartupHealthCheckRetries"),
                "Error message should mention StartupHealthCheckRetries: " + ex.getMessage());
    }

    @Test
    void testInvalidMaxAcceptableLatencyFails() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("http://localhost:2379"))
                .maxAcceptableLatencyMs(0);

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        assertTrue(ex.getMessage().contains("MaxAcceptableLatencyMs"),
                "Error message should mention MaxAcceptableLatencyMs: " + ex.getMessage());
    }

    @Test
    void testMultipleViolationsReportedTogether() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(null)
                .connectTimeoutMs(-1)
                .operationTimeoutMs(-1)
                .tlsEnabled(true)
                .tlsCaPath(null)
                .zkSessionTimeoutMs(100)
                .healthCheckIntervalMs(0)
                .startupHealthCheckTimeoutMs(0)
                .startupHealthCheckRetries(0)
                .maxAcceptableLatencyMs(0);

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class, config::validate);
        String msg = ex.getMessage();

        // All violations should be reported in a single exception message
        assertTrue(msg.contains("Endpoints"), "Should report Endpoints violation");
        assertTrue(msg.contains("ConnectTimeoutMs"), "Should report ConnectTimeoutMs violation");
        assertTrue(msg.contains("OperationTimeoutMs"), "Should report OperationTimeoutMs violation");
        assertTrue(msg.contains("TlsCaPath"), "Should report TlsCaPath violation");
        assertTrue(msg.contains("ZkSessionTimeoutMs"), "Should report ZkSessionTimeoutMs violation");
        assertTrue(msg.contains("HealthCheckIntervalMs"), "Should report HealthCheckIntervalMs violation");
        assertTrue(msg.contains("StartupHealthCheckTimeoutMs"), "Should report StartupHealthCheckTimeoutMs violation");
        assertTrue(msg.contains("StartupHealthCheckRetries"), "Should report StartupHealthCheckRetries violation");
        assertTrue(msg.contains("MaxAcceptableLatencyMs"), "Should report MaxAcceptableLatencyMs violation");
    }

    @Test
    void testValidateReturnsSelfForFluentChaining() {
        StorageConfiguration config = new StorageConfiguration()
                .endpoints(List.of("http://localhost:2379"));

        StorageConfiguration result = config.validate();
        assertTrue(result == config, "validate() should return the same instance for fluent chaining");
    }
}
