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

class CircuitBreakerConfigTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void validEnabledCircuitBreaker() {
        var config = new CircuitBreakerConfig(true, 5, 3, 1, 30000, 100, 50, 1000);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void disabledCircuitBreakerSkipsValidation() {
        var config = new CircuitBreakerConfig(false, 0, 0, 0, 0, 0, 0, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void enabledWithZeroFailureThresholdRejected() {
        var config = new CircuitBreakerConfig(true, 0, 3, 1, 30000, 100, 50, 1000);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void enabledWithZeroSuccessThresholdRejected() {
        var config = new CircuitBreakerConfig(true, 5, 0, 1, 30000, 100, 50, 1000);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void enabledWithZeroHalfOpenRequestsRejected() {
        var config = new CircuitBreakerConfig(true, 5, 3, 0, 30000, 100, 50, 1000);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void enabledWithZeroOpenDurationRejected() {
        var config = new CircuitBreakerConfig(true, 5, 3, 1, 0, 100, 50, 1000);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void enabledWithZeroSlidingWindowRejected() {
        var config = new CircuitBreakerConfig(true, 5, 3, 1, 30000, 0, 50, 1000);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void slowCallThresholdAbove100Rejected() {
        var config = new CircuitBreakerConfig(true, 5, 3, 1, 30000, 100, 101, 1000);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void slowCallThresholdNegativeRejected() {
        var config = new CircuitBreakerConfig(true, 5, 3, 1, 30000, 100, -1, 1000);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativeSlowCallDurationRejected() {
        var config = new CircuitBreakerConfig(true, 5, 3, 1, 30000, 100, 50, -1);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void zeroSlowCallDurationAccepted() {
        var config = new CircuitBreakerConfig(true, 5, 3, 1, 30000, 100, 50, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void jsonRoundTrip() throws Exception {
        var original = new CircuitBreakerConfig(true, 5, 3, 2, 30000, 100, 50, 1000);
        String json = MAPPER.writeValueAsString(original);
        CircuitBreakerConfig deserialized = MAPPER.readValue(json, CircuitBreakerConfig.class);
        assertEquals(original, deserialized);
    }
}
