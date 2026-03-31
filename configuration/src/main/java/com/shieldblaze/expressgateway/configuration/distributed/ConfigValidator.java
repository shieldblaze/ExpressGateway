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

import com.fasterxml.jackson.databind.JsonNode;
import com.shieldblaze.expressgateway.configuration.ConfigurationContext;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Objects;

import static com.shieldblaze.expressgateway.common.JacksonJson.OBJECT_MAPPER;

/**
 * Configuration validator for the distributed configuration system.
 *
 * <p>Performs validation checks on a {@link ConfigurationContext} before it is proposed
 * to the cluster. Validation includes:</p>
 * <ul>
 *   <li>JSON round-trip serialization/deserialization integrity</li>
 *   <li>Required field presence checks (all configuration sub-objects must be non-null)</li>
 *   <li>Cross-field consistency checks (e.g., numeric bounds)</li>
 *   <li>Individual {@link com.shieldblaze.expressgateway.configuration.Configuration#validate()} invocations</li>
 * </ul>
 *
 * <p>Returns a list of validation error strings. An empty list indicates the configuration
 * is valid and safe to propose.</p>
 */
public final class ConfigValidator {

    private static final Logger logger = LogManager.getLogger(ConfigValidator.class);

    private ConfigValidator() {
        // Utility class -- prevent instantiation
    }

    /**
     * Validate a {@link ConfigurationContext} for proposal readiness.
     *
     * @param context The configuration to validate
     * @return An unmodifiable list of validation error messages; empty if valid
     */
    public static List<String> validate(ConfigurationContext context) {
        Objects.requireNonNull(context, "ConfigurationContext must not be null");

        List<String> errors = new ArrayList<>();

        // 1. Required field presence
        validateRequiredFields(context, errors);

        // 2. JSON round-trip integrity
        validateJsonRoundTrip(context, errors);

        // 3. Individual configuration validation
        validateIndividualConfigs(context, errors);

        // 4. Cross-field consistency
        validateCrossFieldConsistency(context, errors);

        if (!errors.isEmpty()) {
            logger.warn("Configuration validation failed with {} error(s): {}", errors.size(), errors);
        } else {
            logger.debug("Configuration validation passed");
        }

        return Collections.unmodifiableList(errors);
    }

    private static void validateRequiredFields(ConfigurationContext context, List<String> errors) {
        if (context.bufferConfiguration() == null) {
            errors.add("bufferConfiguration must not be null");
        }
        if (context.eventLoopConfiguration() == null) {
            errors.add("eventLoopConfiguration must not be null");
        }
        if (context.eventStreamConfiguration() == null) {
            errors.add("eventStreamConfiguration must not be null");
        }
        if (context.healthCheckConfiguration() == null) {
            errors.add("healthCheckConfiguration must not be null");
        }
        if (context.httpConfiguration() == null) {
            errors.add("httpConfiguration must not be null");
        }
        if (context.http3Configuration() == null) {
            errors.add("http3Configuration must not be null");
        }
        if (context.quicConfiguration() == null) {
            errors.add("quicConfiguration must not be null");
        }
        if (context.tlsClientConfiguration() == null) {
            errors.add("tlsClientConfiguration must not be null");
        }
        if (context.tlsServerConfiguration() == null) {
            errors.add("tlsServerConfiguration must not be null");
        }
        if (context.transportConfiguration() == null) {
            errors.add("transportConfiguration must not be null");
        }
    }

    private static void validateJsonRoundTrip(ConfigurationContext context, List<String> errors) {
        try {
            String json = OBJECT_MAPPER.writeValueAsString(context);

            // Verify it is valid JSON
            JsonNode tree = OBJECT_MAPPER.readTree(json);
            if (tree == null || tree.isEmpty()) {
                errors.add("JSON serialization produced null or empty document");
                return;
            }

            // Verify round-trip deserialization
            ConfigurationContext roundTrip = OBJECT_MAPPER.readValue(json, ConfigurationContext.class);
            if (roundTrip == null) {
                errors.add("JSON round-trip deserialization produced null");
            }
        } catch (Exception e) {
            errors.add("JSON round-trip validation failed: " + e.getMessage());
        }
    }

    private static void validateIndividualConfigs(ConfigurationContext context, List<String> errors) {
        // Each Configuration implementation has a validate() method that throws
        // IllegalArgumentException on failure. We catch and collect those.

        tryValidate("bufferConfiguration", context.bufferConfiguration(), errors);
        tryValidate("eventLoopConfiguration", context.eventLoopConfiguration(), errors);
        tryValidate("eventStreamConfiguration", context.eventStreamConfiguration(), errors);
        tryValidate("healthCheckConfiguration", context.healthCheckConfiguration(), errors);
        tryValidate("httpConfiguration", context.httpConfiguration(), errors);
        tryValidate("http3Configuration", context.http3Configuration(), errors);
        tryValidate("quicConfiguration", context.quicConfiguration(), errors);
        tryValidate("tlsClientConfiguration", context.tlsClientConfiguration(), errors);
        tryValidate("tlsServerConfiguration", context.tlsServerConfiguration(), errors);
        tryValidate("transportConfiguration", context.transportConfiguration(), errors);
    }

    private static void tryValidate(String name,
                                    com.shieldblaze.expressgateway.configuration.Configuration<?> config,
                                    List<String> errors) {
        if (config == null) {
            // Already reported by validateRequiredFields
            return;
        }
        try {
            config.validate();
        } catch (IllegalArgumentException | IllegalStateException e) {
            errors.add(name + " validation failed: " + e.getMessage());
        } catch (Exception e) {
            errors.add(name + " validation threw unexpected exception: " + e.getClass().getSimpleName()
                    + ": " + e.getMessage());
        }
    }

    private static void validateCrossFieldConsistency(ConfigurationContext context, List<String> errors) {
        // HTTP/3 requires QUIC to be properly configured
        if (context.http3Configuration() != null && context.quicConfiguration() == null) {
            errors.add("http3Configuration is present but quicConfiguration is null -- HTTP/3 requires QUIC");
        }

        // TLS server is required for HTTP/3/QUIC (QUIC mandates TLS 1.3)
        if (context.quicConfiguration() != null && context.tlsServerConfiguration() == null) {
            errors.add("quicConfiguration is present but tlsServerConfiguration is null -- QUIC mandates TLS");
        }
    }
}
