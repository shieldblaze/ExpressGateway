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
package com.shieldblaze.expressgateway.configuration.transport;

import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.MethodOrderer;
import org.junit.jupiter.api.Order;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.TestMethodOrder;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Tests for {@link BackendProxyProtocolMode} and its integration with
 * {@link TransportConfiguration}.
 *
 * <p>Covers:
 * <ul>
 *   <li>Enum value existence and ordering</li>
 *   <li>Default value in {@link TransportConfiguration} is {@link BackendProxyProtocolMode#OFF}</li>
 *   <li>Set / get round-trip for all three modes on {@link TransportConfiguration}</li>
 *   <li>Null-safety: {@link TransportConfiguration#backendProxyProtocolMode()} returns
 *       {@link BackendProxyProtocolMode#OFF} when the underlying field is {@code null}
 *       (supports forward-compatible JSON deserialization)</li>
 *   <li>JSON serialization/deserialization round-trip via Jackson {@link ObjectMapper}</li>
 *   <li>{@link TransportConfiguration.DEFAULT} is pre-validated and has the correct default</li>
 * </ul>
 */
@TestMethodOrder(MethodOrderer.OrderAnnotation.class)
class BackendProxyProtocolModeTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    // =========================================================================
    // 1. Enum structure
    // =========================================================================

    @Order(1)
    @Test
    void enumHasExactlyThreeValues() {
        BackendProxyProtocolMode[] values = BackendProxyProtocolMode.values();
        assertEquals(3, values.length,
                "BackendProxyProtocolMode must have exactly 3 values: OFF, V1, V2");
    }

    @Order(2)
    @Test
    void enumValuesAreOFFV1V2() {
        // Verify valueOf works for all three names (fails with IllegalArgumentException if missing)
        assertEquals(BackendProxyProtocolMode.OFF, BackendProxyProtocolMode.valueOf("OFF"));
        assertEquals(BackendProxyProtocolMode.V1,  BackendProxyProtocolMode.valueOf("V1"));
        assertEquals(BackendProxyProtocolMode.V2,  BackendProxyProtocolMode.valueOf("V2"));
    }

    @Order(3)
    @Test
    void enumDoesNotHaveAUTOMode() {
        // Unlike the inbound ProxyProtocolMode, BackendProxyProtocolMode must NOT
        // have AUTO — the sender must always choose a specific version.
        assertThrows(IllegalArgumentException.class,
                () -> BackendProxyProtocolMode.valueOf("AUTO"),
                "BackendProxyProtocolMode must not have an AUTO value (outbound encoding requires an explicit version)");
    }

    // =========================================================================
    // 2. TransportConfiguration defaults
    // =========================================================================

    @Order(4)
    @Test
    void defaultTransportConfigurationHasOffMode() {
        assertEquals(BackendProxyProtocolMode.OFF,
                TransportConfiguration.DEFAULT.backendProxyProtocolMode(),
                "TransportConfiguration.DEFAULT must have backendProxyProtocolMode=OFF " +
                        "to ensure backward compatibility with existing deployments");
    }

    @Order(5)
    @Test
    void defaultTransportConfigurationIsPreValidated() {
        assertTrue(TransportConfiguration.DEFAULT.validated(),
                "TransportConfiguration.DEFAULT must be pre-validated");
    }

    // =========================================================================
    // 3. Set / get round-trip on a new TransportConfiguration
    // =========================================================================

    @Order(6)
    @Test
    void setAndGetOffMode() {
        TransportConfiguration config = buildValidConfig();
        config.setBackendProxyProtocolMode(BackendProxyProtocolMode.OFF);
        config.validate();

        assertEquals(BackendProxyProtocolMode.OFF, config.backendProxyProtocolMode(),
                "Set OFF must return OFF after validate()");
    }

    @Order(7)
    @Test
    void setAndGetV1Mode() {
        TransportConfiguration config = buildValidConfig();
        config.setBackendProxyProtocolMode(BackendProxyProtocolMode.V1);
        config.validate();

        assertEquals(BackendProxyProtocolMode.V1, config.backendProxyProtocolMode(),
                "Set V1 must return V1 after validate()");
    }

    @Order(8)
    @Test
    void setAndGetV2Mode() {
        TransportConfiguration config = buildValidConfig();
        config.setBackendProxyProtocolMode(BackendProxyProtocolMode.V2);
        config.validate();

        assertEquals(BackendProxyProtocolMode.V2, config.backendProxyProtocolMode(),
                "Set V2 must return V2 after validate()");
    }

    // =========================================================================
    // 4. Null-safety: backendProxyProtocolMode() returns OFF when field is null
    // =========================================================================

    @Order(9)
    @Test
    void backendProxyProtocolModeReturnsOffWhenFieldIsNull() throws Exception {
        // Simulate forward-compatible deserialization: a config JSON written before
        // backendProxyProtocolMode was added will not have the field, leaving it null
        // after Jackson deserialization. The getter must return OFF (not throw NPE).
        String json = "{}";
        TransportConfiguration config = MAPPER.readValue(json, TransportConfiguration.class);

        assertEquals(BackendProxyProtocolMode.OFF, config.backendProxyProtocolMode(),
                "backendProxyProtocolMode() must return OFF (not null/NPE) when the JSON field is absent");
    }

    // =========================================================================
    // 5. JSON serialization round-trip (Jackson ObjectMapper)
    // =========================================================================

    @Order(10)
    @Test
    void jsonRoundTripSerializesOffCorrectly() throws Exception {
        TransportConfiguration config = buildValidConfig();
        config.setBackendProxyProtocolMode(BackendProxyProtocolMode.OFF);

        String json = MAPPER.writeValueAsString(config);
        assertTrue(json.contains("\"backendProxyProtocolMode\""),
                "Serialized JSON must contain 'backendProxyProtocolMode' key");
        assertTrue(json.contains("\"OFF\""),
                "Serialized JSON must contain 'OFF' value for backendProxyProtocolMode");

        TransportConfiguration deserialized = MAPPER.readValue(json, TransportConfiguration.class);
        assertEquals(BackendProxyProtocolMode.OFF, deserialized.backendProxyProtocolMode(),
                "Deserialized OFF mode must round-trip correctly");
    }

    @Order(11)
    @Test
    void jsonRoundTripSerializesV1Correctly() throws Exception {
        TransportConfiguration config = buildValidConfig();
        config.setBackendProxyProtocolMode(BackendProxyProtocolMode.V1);

        String json = MAPPER.writeValueAsString(config);
        assertTrue(json.contains("\"V1\""),
                "Serialized JSON must contain 'V1' for backendProxyProtocolMode");

        TransportConfiguration deserialized = MAPPER.readValue(json, TransportConfiguration.class);
        assertEquals(BackendProxyProtocolMode.V1, deserialized.backendProxyProtocolMode(),
                "Deserialized V1 mode must round-trip correctly");
    }

    @Order(12)
    @Test
    void jsonRoundTripSerializesV2Correctly() throws Exception {
        TransportConfiguration config = buildValidConfig();
        config.setBackendProxyProtocolMode(BackendProxyProtocolMode.V2);

        String json = MAPPER.writeValueAsString(config);
        assertTrue(json.contains("\"V2\""),
                "Serialized JSON must contain 'V2' for backendProxyProtocolMode");

        TransportConfiguration deserialized = MAPPER.readValue(json, TransportConfiguration.class);
        assertEquals(BackendProxyProtocolMode.V2, deserialized.backendProxyProtocolMode(),
                "Deserialized V2 mode must round-trip correctly");
    }

    @Order(13)
    @Test
    void jsonDeserializationWithExplicitNullFieldReturnsOff() throws Exception {
        // Explicit null in JSON (e.g., a PATCH operation that unsets the field)
        // must result in the getter returning OFF per the null-safety contract.
        String json = "{\"backendProxyProtocolMode\": null}";
        TransportConfiguration config = MAPPER.readValue(json, TransportConfiguration.class);

        assertEquals(BackendProxyProtocolMode.OFF, config.backendProxyProtocolMode(),
                "Explicit null in JSON for backendProxyProtocolMode must return OFF");
    }

    // =========================================================================
    // 6. TransportConfiguration.validate() accepts all three modes
    // =========================================================================

    @Order(14)
    @Test
    void validateAcceptsAllThreeModes() {
        for (BackendProxyProtocolMode mode : BackendProxyProtocolMode.values()) {
            TransportConfiguration config = buildValidConfig();
            config.setBackendProxyProtocolMode(mode);
            assertDoesNotThrow(config::validate,
                    "validate() must accept backendProxyProtocolMode=" + mode);
            assertEquals(mode, config.backendProxyProtocolMode(),
                    "Mode " + mode + " must survive validate()");
        }
    }

    // =========================================================================
    // 7. backendProxyProtocolMode is independent of proxyProtocolMode (inbound)
    // =========================================================================

    @Order(15)
    @Test
    void backendProxyProtocolModeIsIndependentOfInboundMode() {
        // Setting backendProxyProtocolMode=V2 must not change proxyProtocolMode (inbound)
        TransportConfiguration config = buildValidConfig();
        config.setProxyProtocolMode(ProxyProtocolMode.OFF);
        config.setBackendProxyProtocolMode(BackendProxyProtocolMode.V2);
        config.validate();

        assertEquals(ProxyProtocolMode.OFF, config.proxyProtocolMode(),
                "Setting backendProxyProtocolMode must not change the inbound proxyProtocolMode");
        assertEquals(BackendProxyProtocolMode.V2, config.backendProxyProtocolMode(),
                "backendProxyProtocolMode must retain its configured value");
    }

    @Order(16)
    @Test
    void bothModesCanBeSetIndependently() {
        // Both inbound and outbound modes can be configured simultaneously
        TransportConfiguration config = buildValidConfig();
        config.setProxyProtocolMode(ProxyProtocolMode.AUTO);
        config.setBackendProxyProtocolMode(BackendProxyProtocolMode.V1);
        config.validate();

        assertEquals(ProxyProtocolMode.AUTO, config.proxyProtocolMode(),
                "Inbound proxyProtocolMode must retain AUTO");
        assertEquals(BackendProxyProtocolMode.V1, config.backendProxyProtocolMode(),
                "backendProxyProtocolMode must retain V1");
    }

    // =========================================================================
    // Helper
    // =========================================================================

    /**
     * Builds a {@link TransportConfiguration} with all required fields set to
     * valid values (NIO transport type, adaptive buffer allocation). The caller
     * may override specific fields before calling {@code validate()}.
     */
    private static TransportConfiguration buildValidConfig() {
        return new TransportConfiguration()
                .setTransportType(TransportType.NIO)
                .setReceiveBufferAllocationType(ReceiveBufferAllocationType.ADAPTIVE)
                .setReceiveBufferSizes(new int[]{512, 9001, 65535})
                .setTcpConnectionBacklog(1024)
                .setSocketReceiveBufferSize(65536)
                .setSocketSendBufferSize(65536)
                .setTcpFastOpenMaximumPendingRequests(100)
                .setBackendConnectTimeout(5_000)
                .setConnectionIdleTimeout(60_000)
                .setProxyProtocolMode(ProxyProtocolMode.OFF)
                .setBackendProxyProtocolMode(BackendProxyProtocolMode.OFF);
    }
}
