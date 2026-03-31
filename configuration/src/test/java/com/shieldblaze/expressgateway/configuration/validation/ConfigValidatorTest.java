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
package com.shieldblaze.expressgateway.configuration.validation;

import com.shieldblaze.expressgateway.configuration.schema.BackendConfig;
import com.shieldblaze.expressgateway.configuration.schema.CircuitBreakerConfig;
import com.shieldblaze.expressgateway.configuration.schema.ListenerConfig;
import com.shieldblaze.expressgateway.configuration.schema.QuicPolicyConfig;
import com.shieldblaze.expressgateway.configuration.schema.RateLimitConfig;
import com.shieldblaze.expressgateway.configuration.schema.RetryPolicyConfig;
import com.shieldblaze.expressgateway.configuration.schema.RouteConfig;
import com.shieldblaze.expressgateway.configuration.schema.TrafficShapingConfig;
import com.shieldblaze.expressgateway.configuration.validation.ValidationResult.ValidationError;
import org.junit.jupiter.api.Test;

import java.util.List;
import java.util.Set;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ConfigValidatorTest {

    @Test
    void validListenerReturnsSuccess() {
        var config = new ListenerConfig("web", "0.0.0.0", 8080, "TCP", null, 10000, 60000);
        var result = ConfigValidator.validate(config);
        assertTrue(result.valid());
        assertTrue(result.errors().isEmpty());
    }

    @Test
    void invalidListenerReturnsError() {
        var config = new ListenerConfig("web", "0.0.0.0", 0, "TCP", null, 10000, 60000);
        var result = ConfigValidator.validate(config);
        assertFalse(result.valid());
        assertFalse(result.errors().isEmpty());
    }

    @Test
    void validRouteReturnsSuccess() {
        var config = new RouteConfig("route", "host.com", null, null, "cluster", 0, null, 0);
        var result = ConfigValidator.validate(config, Set.of("cluster"));
        assertTrue(result.valid());
    }

    @Test
    void routeWithUnknownBackendReturnsError() {
        var config = new RouteConfig("route", "host.com", null, null, "unknown-cluster", 0, null, 0);
        var result = ConfigValidator.validate(config, Set.of("cluster-a", "cluster-b"));
        assertFalse(result.valid());
        assertTrue(result.errors().stream().anyMatch(e -> e.field().equals("backendClusterRef")));
    }

    @Test
    void routeWithEmptyKnownBackendsSkipsRefCheck() {
        var config = new RouteConfig("route", "host.com", null, null, "any-cluster", 0, null, 0);
        var result = ConfigValidator.validate(config, Set.of());
        assertTrue(result.valid());
    }

    @Test
    void validBackendReturnsSuccess() {
        var config = new BackendConfig("cluster", List.of("10.0.0.1:80"), "round-robin", null, null, 100, 0);
        var result = ConfigValidator.validate(config);
        assertTrue(result.valid());
    }

    @Test
    void backendWithDuplicateNodesReturnsWarning() {
        var config = new BackendConfig("cluster", List.of("10.0.0.1:80", "10.0.0.1:80"),
                "round-robin", null, null, 100, 0);
        var result = ConfigValidator.validate(config);
        // Valid because warnings don't cause failure
        assertTrue(result.valid());
        assertTrue(result.errors().stream()
                .anyMatch(e -> e.severity() == ValidationError.Severity.WARNING && e.field().equals("nodes")));
    }

    @Test
    void invalidBackendReturnsError() {
        var config = new BackendConfig("cluster", List.of(), "round-robin", null, null, 100, 0);
        var result = ConfigValidator.validate(config);
        assertFalse(result.valid());
    }

    @Test
    void validRateLimitReturnsSuccess() {
        var config = new RateLimitConfig("default", 100, 200, "IP", null, 429, 60);
        var result = ConfigValidator.validate(config);
        assertTrue(result.valid());
    }

    @Test
    void retryPolicyWithExcessiveTotalTimeReturnsWarning() {
        var config = new RetryPolicyConfig(5, List.of("502"), "fixed", 10000, 5000);
        // Route timeout of 10000ms, total retry = 5000 * (5+1) = 30000ms > 10000ms
        var result = ConfigValidator.validate(config, 10000);
        assertTrue(result.valid()); // Warning, not error
        assertTrue(result.errors().stream()
                .anyMatch(e -> e.severity() == ValidationError.Severity.WARNING));
    }

    @Test
    void retryPolicyWithinTimeoutReturnsClean() {
        var config = new RetryPolicyConfig(2, List.of("502"), "fixed", 5000, 1000);
        // Route timeout of 10000ms, total retry = 1000 * (2+1) = 3000ms < 10000ms
        var result = ConfigValidator.validate(config, 10000);
        assertTrue(result.valid());
        assertTrue(result.errors().isEmpty());
    }

    @Test
    void retryPolicyZeroRouteTimeoutSkipsCrossFieldCheck() {
        var config = new RetryPolicyConfig(5, List.of("502"), "fixed", 10000, 5000);
        var result = ConfigValidator.validate(config, 0);
        assertTrue(result.valid());
    }

    @Test
    void validCircuitBreakerReturnsSuccess() {
        var config = new CircuitBreakerConfig(true, 5, 3, 1, 30000, 100, 50, 1000);
        var result = ConfigValidator.validate(config);
        assertTrue(result.valid());
    }

    @Test
    void validTrafficShapingReturnsSuccess() {
        var config = new TrafficShapingConfig("default", 1_000_000, 100_000, 500_000, 800_000, 800_000);
        var result = ConfigValidator.validate(config);
        assertTrue(result.valid());
    }

    @Test
    void validQuicPolicyReturnsSuccess() {
        var config = new QuicPolicyConfig(30000, 1350, 10_000_000, 1_000_000, 1_000_000,
                1_000_000, 100, 3, 3, 25, false, 8);
        var result = ConfigValidator.validate(config);
        assertTrue(result.valid());
    }

    @Test
    void quicPolicyWithLowUniStreamsReturnsWarning() {
        var config = new QuicPolicyConfig(30000, 1350, 10_000_000, 1_000_000, 1_000_000,
                1_000_000, 100, 2, 3, 25, false, 8);
        var result = ConfigValidator.validate(config);
        assertTrue(result.valid()); // Warning, not error
        assertTrue(result.errors().stream()
                .anyMatch(e -> e.severity() == ValidationError.Severity.WARNING
                        && e.field().equals("initialMaxStreamsUni")));
    }

    @Test
    void quicListenerWithoutQuicPolicyReturnsError() {
        var listener = new ListenerConfig("quic-web", "0.0.0.0", 443, "QUIC", "cert", 10000, 30000);
        var result = ConfigValidator.validateListenerWithQuic(listener, null);
        assertFalse(result.valid());
    }

    @Test
    void quicListenerWithQuicPolicyReturnsSuccess() {
        var listener = new ListenerConfig("quic-web", "0.0.0.0", 443, "QUIC", "cert", 10000, 30000);
        var quicPolicy = new QuicPolicyConfig(30000, 1350, 10_000_000, 1_000_000, 1_000_000,
                1_000_000, 100, 3, 3, 25, false, 8);
        var result = ConfigValidator.validateListenerWithQuic(listener, quicPolicy);
        assertTrue(result.valid());
    }

    @Test
    void validationResultSummary() {
        var success = ValidationResult.success();
        assertEquals("valid", success.summary());

        var withErrors = ValidationResult.of(List.of(
                ValidationError.error("field1", "error1"),
                ValidationError.warning("field2", "warn1"),
                ValidationError.info("field3", "info1")
        ));
        assertEquals("errors=1, warnings=1, info=1", withErrors.summary());
    }
}
