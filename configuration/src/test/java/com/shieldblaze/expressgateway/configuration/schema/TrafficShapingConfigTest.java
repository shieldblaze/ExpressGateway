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

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class TrafficShapingConfigTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void validTrafficShaping() {
        var config = new TrafficShapingConfig("default", 1_000_000, 100_000, 500_000, 800_000, 800_000);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void unlimitedBandwidthAccepted() {
        var config = new TrafficShapingConfig("unlimited", 0, 0, 0, 0, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void negativeBandwidthRejected() {
        var config = new TrafficShapingConfig("bad", -1, 0, 0, 0, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeBurstRejected() {
        var config = new TrafficShapingConfig("bad", 0, -1, 0, 0, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativePerConnectionLimitRejected() {
        var config = new TrafficShapingConfig("bad", 0, 0, -1, 0, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeReadLimitRejected() {
        var config = new TrafficShapingConfig("bad", 0, 0, 0, -1, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeWriteLimitRejected() {
        var config = new TrafficShapingConfig("bad", 0, 0, 0, 0, -1);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void perConnectionExceedsGlobalRejected() {
        var config = new TrafficShapingConfig("bad", 1000, 0, 2000, 0, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void perConnectionEqualsGlobalAccepted() {
        var config = new TrafficShapingConfig("ok", 1000, 0, 1000, 0, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void perConnectionUnlimitedWithGlobalLimitAccepted() {
        var config = new TrafficShapingConfig("ok", 1000, 0, 0, 0, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void blankNameRejected() {
        var config = new TrafficShapingConfig("", 0, 0, 0, 0, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void jsonRoundTrip() throws Exception {
        var original = new TrafficShapingConfig("shaping", 1000000, 10000, 500000, 800000, 700000);
        String json = MAPPER.writeValueAsString(original);
        TrafficShapingConfig deserialized = MAPPER.readValue(json, TrafficShapingConfig.class);
        assertEquals(original, deserialized);
    }
}
