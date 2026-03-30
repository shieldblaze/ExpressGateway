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

import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class RouteConfigTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void validHostMatch() {
        var config = new RouteConfig("api-route", "api.example.com", null, null, "api-cluster", 10, null, 5000);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void validPathMatch() {
        var config = new RouteConfig("path-route", null, "/api/v1", null, "api-cluster", 10, null, 5000);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void validHeaderMatch() {
        var config = new RouteConfig("header-route", null, null, Map.of("X-Version", "v2"), "api-cluster", 5, null, 3000);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void validCombinedMatch() {
        var config = new RouteConfig("combo-route", "example.com", "/api", Map.of("Accept", "application/json"),
                "web-cluster", 1, "retry-default", 10000);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void noMatchCriteriaRejected() {
        var config = new RouteConfig("empty-route", null, null, null, "cluster", 0, null, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void blankMatchCriteriaRejected() {
        var config = new RouteConfig("empty-route", "", "", Map.of(), "cluster", 0, null, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void pathNotStartingWithSlashRejected() {
        var config = new RouteConfig("bad-path", null, "api/v1", null, "cluster", 0, null, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void blankNameRejected() {
        var config = new RouteConfig("", "host", null, null, "cluster", 0, null, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void blankBackendRefRejected() {
        var config = new RouteConfig("route", "host", null, null, "", 0, null, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativePriorityRejected() {
        var config = new RouteConfig("route", "host", null, null, "cluster", -1, null, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeTimeoutRejected() {
        var config = new RouteConfig("route", "host", null, null, "cluster", 0, null, -1);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void matchHeadersDefensivelyCopied() {
        var mutable = new java.util.HashMap<String, String>();
        mutable.put("X-Test", "value");
        var config = new RouteConfig("route", null, null, mutable, "cluster", 0, null, 0);
        mutable.put("X-Extra", "extra");
        assertEquals(1, config.matchHeaders().size());
    }

    @Test
    void jsonRoundTrip() throws Exception {
        var original = new RouteConfig("route", "host.com", "/path", Map.of("h", "v"), "cluster", 5, "retry", 3000);
        String json = MAPPER.writeValueAsString(original);
        RouteConfig deserialized = MAPPER.readValue(json, RouteConfig.class);
        assertEquals(original, deserialized);
    }
}
