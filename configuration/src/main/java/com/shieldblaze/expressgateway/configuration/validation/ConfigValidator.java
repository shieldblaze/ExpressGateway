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

import java.util.ArrayList;
import java.util.Collection;
import java.util.HashSet;
import java.util.List;
import java.util.Objects;
import java.util.Set;

/**
 * Validates configuration schema records with field-level, cross-field,
 * and cross-reference validation.
 *
 * <p>This validator complements each record's own {@code validate()} method
 * by performing higher-level checks:</p>
 * <ul>
 *   <li>Cross-field validation (e.g., retry timeout < total timeout)</li>
 *   <li>Reference validation (e.g., backend cluster exists)</li>
 *   <li>Semantic validation (e.g., QUIC requires TLS listener)</li>
 * </ul>
 */
public final class ConfigValidator {

    private ConfigValidator() {
        // Static utility class
    }

    /**
     * Validate a listener configuration.
     *
     * @param config The listener config to validate
     * @return The validation result
     */
    public static ValidationResult validate(ListenerConfig config) {
        Objects.requireNonNull(config, "config");
        List<ValidationError> errors = new ArrayList<>();
        try {
            config.validate();
        } catch (IllegalArgumentException | NullPointerException e) {
            errors.add(ValidationError.error("listener", e.getMessage()));
        }
        return ValidationResult.of(errors);
    }

    /**
     * Validate a route configuration, optionally checking that the backend cluster reference exists.
     *
     * @param config           The route config to validate
     * @param knownBackendRefs Known backend cluster names for reference validation (may be empty)
     * @return The validation result
     */
    public static ValidationResult validate(RouteConfig config, Set<String> knownBackendRefs) {
        Objects.requireNonNull(config, "config");
        List<ValidationError> errors = new ArrayList<>();
        try {
            config.validate();
        } catch (IllegalArgumentException | NullPointerException e) {
            errors.add(ValidationError.error("route", e.getMessage()));
        }
        // Cross-reference validation
        if (knownBackendRefs != null && !knownBackendRefs.isEmpty()
                && config.backendClusterRef() != null
                && !knownBackendRefs.contains(config.backendClusterRef())) {
            errors.add(ValidationError.error("backendClusterRef",
                    "Backend cluster '" + config.backendClusterRef() + "' not found in known backends"));
        }
        return ValidationResult.of(errors);
    }

    /**
     * Validate a backend configuration.
     *
     * @param config The backend config to validate
     * @return The validation result
     */
    public static ValidationResult validate(BackendConfig config) {
        Objects.requireNonNull(config, "config");
        List<ValidationError> errors = new ArrayList<>();
        try {
            config.validate();
        } catch (IllegalArgumentException | NullPointerException e) {
            errors.add(ValidationError.error("backend", e.getMessage()));
        }
        // Warn on duplicate nodes
        Set<String> seen = new HashSet<>();
        for (String node : config.nodes()) {
            if (node != null && !seen.add(node)) {
                errors.add(ValidationError.warning("nodes", "Duplicate node address: " + node));
            }
        }
        return ValidationResult.of(errors);
    }

    /**
     * Validate a rate limit configuration.
     *
     * @param config The rate limit config to validate
     * @return The validation result
     */
    public static ValidationResult validate(RateLimitConfig config) {
        Objects.requireNonNull(config, "config");
        List<ValidationError> errors = new ArrayList<>();
        try {
            config.validate();
        } catch (IllegalArgumentException | NullPointerException e) {
            errors.add(ValidationError.error("rateLimit", e.getMessage()));
        }
        return ValidationResult.of(errors);
    }

    /**
     * Validate a retry policy configuration with optional cross-field checking
     * against a route's total timeout.
     *
     * @param config         The retry policy config to validate
     * @param routeTimeoutMs The route's total timeout in ms (0 to skip cross-field check)
     * @return The validation result
     */
    public static ValidationResult validate(RetryPolicyConfig config, long routeTimeoutMs) {
        Objects.requireNonNull(config, "config");
        List<ValidationError> errors = new ArrayList<>();
        try {
            config.validate();
        } catch (IllegalArgumentException | NullPointerException e) {
            errors.add(ValidationError.error("retryPolicy", e.getMessage()));
        }
        // Cross-field: total retry time should not exceed route timeout
        if (routeTimeoutMs > 0 && config.perTryTimeoutMs() > 0 && config.maxRetries() > 0) {
            long totalRetryTime = config.perTryTimeoutMs() * (config.maxRetries() + 1);
            if (totalRetryTime > routeTimeoutMs) {
                errors.add(ValidationError.warning("perTryTimeoutMs",
                        "Total retry time (" + totalRetryTime + "ms) exceeds route timeout (" + routeTimeoutMs + "ms)"));
            }
        }
        return ValidationResult.of(errors);
    }

    /**
     * Validate a circuit breaker configuration.
     *
     * @param config The circuit breaker config to validate
     * @return The validation result
     */
    public static ValidationResult validate(CircuitBreakerConfig config) {
        Objects.requireNonNull(config, "config");
        List<ValidationError> errors = new ArrayList<>();
        try {
            config.validate();
        } catch (IllegalArgumentException | NullPointerException e) {
            errors.add(ValidationError.error("circuitBreaker", e.getMessage()));
        }
        return ValidationResult.of(errors);
    }

    /**
     * Validate a traffic shaping configuration.
     *
     * @param config The traffic shaping config to validate
     * @return The validation result
     */
    public static ValidationResult validate(TrafficShapingConfig config) {
        Objects.requireNonNull(config, "config");
        List<ValidationError> errors = new ArrayList<>();
        try {
            config.validate();
        } catch (IllegalArgumentException | NullPointerException e) {
            errors.add(ValidationError.error("trafficShaping", e.getMessage()));
        }
        return ValidationResult.of(errors);
    }

    /**
     * Validate a QUIC policy configuration.
     *
     * @param config The QUIC policy config to validate
     * @return The validation result
     */
    public static ValidationResult validate(QuicPolicyConfig config) {
        Objects.requireNonNull(config, "config");
        List<ValidationError> errors = new ArrayList<>();
        try {
            config.validate();
        } catch (IllegalArgumentException | NullPointerException e) {
            errors.add(ValidationError.error("quicPolicy", e.getMessage()));
        }
        // HTTP/3 requires at least 3 unidirectional streams for control, QPACK encoder, QPACK decoder
        if (config.initialMaxStreamsUni() > 0 && config.initialMaxStreamsUni() < 3) {
            errors.add(ValidationError.warning("initialMaxStreamsUni",
                    "HTTP/3 requires at least 3 unidirectional streams (RFC 9114 Section 6.2), got: " + config.initialMaxStreamsUni()));
        }
        return ValidationResult.of(errors);
    }

    /**
     * Validate a listener configuration against a QUIC policy, ensuring semantic consistency.
     *
     * @param listener   The listener configuration
     * @param quicPolicy The QUIC policy (may be null if no QUIC is used)
     * @return The validation result
     */
    public static ValidationResult validateListenerWithQuic(ListenerConfig listener, QuicPolicyConfig quicPolicy) {
        Objects.requireNonNull(listener, "listener");
        List<ValidationError> errors = new ArrayList<>();

        if ("QUIC".equals(listener.protocol()) && quicPolicy == null) {
            errors.add(ValidationError.error("protocol",
                    "Listener uses QUIC protocol but no QuicPolicyConfig is provided"));
        }

        return ValidationResult.of(errors);
    }
}
