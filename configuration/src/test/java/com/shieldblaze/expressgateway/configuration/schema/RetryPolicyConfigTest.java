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

class RetryPolicyConfigTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();

    @Test
    void validRetryPolicy() {
        var config = new RetryPolicyConfig(3, List.of("502", "503", "connect-failure"), "exponential", 5000, 2000);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void zeroRetriesNoConditionsAccepted() {
        var config = new RetryPolicyConfig(0, List.of(), "fixed", 0, 0);
        assertDoesNotThrow(config::validate);
    }

    @Test
    void negativeMaxRetriesRejected() {
        var config = new RetryPolicyConfig(-1, List.of("502"), "fixed", 0, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void retriesWithoutConditionsRejected() {
        var config = new RetryPolicyConfig(3, List.of(), "fixed", 0, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void invalidRetryConditionRejected() {
        var config = new RetryPolicyConfig(3, List.of("500"), "fixed", 0, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void invalidBackoffStrategyRejected() {
        var config = new RetryPolicyConfig(3, List.of("502"), "linear", 0, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void allBackoffStrategiesAccepted() {
        for (String strategy : List.of("fixed", "exponential", "jittered")) {
            var config = new RetryPolicyConfig(1, List.of("502"), strategy, 1000, 500);
            assertDoesNotThrow(config::validate, "Strategy should be accepted: " + strategy);
        }
    }

    @Test
    void allRetryConditionsAccepted() {
        for (String condition : List.of("502", "503", "504", "connect-failure", "timeout", "reset")) {
            var config = new RetryPolicyConfig(1, List.of(condition), "fixed", 0, 0);
            assertDoesNotThrow(config::validate, "Condition should be accepted: " + condition);
        }
    }

    @Test
    void negativeMaxBackoffRejected() {
        var config = new RetryPolicyConfig(1, List.of("502"), "fixed", -1, 0);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void negativePerTryTimeoutRejected() {
        var config = new RetryPolicyConfig(1, List.of("502"), "fixed", 0, -1);
        assertThrows(IllegalArgumentException.class, config::validate);
    }

    @Test
    void retryOnDefensivelyCopied() {
        var mutable = new java.util.ArrayList<>(List.of("502"));
        var config = new RetryPolicyConfig(1, mutable, "fixed", 0, 0);
        mutable.add("503");
        assertEquals(1, config.retryOn().size());
    }

    @Test
    void jsonRoundTrip() throws Exception {
        var original = new RetryPolicyConfig(3, List.of("502", "timeout"), "exponential", 5000, 2000);
        String json = MAPPER.writeValueAsString(original);
        RetryPolicyConfig deserialized = MAPPER.readValue(json, RetryPolicyConfig.class);
        assertEquals(original, deserialized);
    }
}
