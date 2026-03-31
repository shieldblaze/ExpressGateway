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
package com.shieldblaze.expressgateway.configuration.schema;

import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class BackendConfigTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void validBackendConfig() {
        var config = new BackendConfig("api-cluster", List.of("10.0.0.1:8080", "10.0.0.2:8080"),
                "round-robin", "hc-default", "cb-default", 100, 10);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void allStrategiesAccepted() {
        for (String strategy : List.of("round-robin", "least-connection", "random", "ip-hash", "weighted-round-robin")) {
            var config = new BackendConfig("cluster", List.of("10.0.0.1:80"), strategy, null, null, 1, 0);
            assertDoesNotThrow(config::validate, "Strategy should be accepted: " + strategy);
        }
    }

    @Test
    void emptyNodesRejected() {
        var config = new BackendConfig("cluster", List.of(), "round-robin", null, null, 100, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void nullNodesBecomesEmptyList() {
        var config = new BackendConfig("cluster", null, "round-robin", null, null, 100, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
        assertEquals(List.of(), config.nodes());
    }

    @Test
    void blankNodeRejected() {
        var config = new BackendConfig("cluster", List.of("10.0.0.1:80", ""), "round-robin", null, null, 100, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void invalidNodeFormatRejected() {
        var config = new BackendConfig("cluster", List.of("10.0.0.1"), "round-robin", null, null, 100, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void invalidNodePortRejected() {
        var config = new BackendConfig("cluster", List.of("10.0.0.1:abc"), "round-robin", null, null, 100, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void nodePortOutOfRangeRejected() {
        var config = new BackendConfig("cluster", List.of("10.0.0.1:70000"), "round-robin", null, null, 100, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void invalidStrategyRejected() {
        var config = new BackendConfig("cluster", List.of("10.0.0.1:80"), "unknown", null, null, 100, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void zeroMaxConnectionsRejected() {
        var config = new BackendConfig("cluster", List.of("10.0.0.1:80"), "round-robin", null, null, 0, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeConnectionPoolSizeRejected() {
        var config = new BackendConfig("cluster", List.of("10.0.0.1:80"), "round-robin", null, null, 100, -1);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void nodesDefensivelyCopied() {
        var mutable = new java.util.ArrayList<>(List.of("10.0.0.1:80"));
        var config = new BackendConfig("cluster", mutable, "round-robin", null, null, 100, 0);
        mutable.add("10.0.0.2:80");
        assertEquals(1, config.nodes().size());
    }

    @Test
    void jsonRoundTrip() throws Exception {
        var original = new BackendConfig("cluster", List.of("10.0.0.1:80"), "round-robin", "hc", "cb", 100, 10);
        String json = MAPPER.writeValueAsString(original);
        BackendConfig deserialized = MAPPER.readValue(json, BackendConfig.class);
        assertEquals(original, deserialized);
    }
}
