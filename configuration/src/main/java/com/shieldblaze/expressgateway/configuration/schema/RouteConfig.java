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

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * Configuration schema for an L7 routing rule.
 *
 * @param name               The rule name
 * @param matchHost          Host match pattern (e.g. "example.com", "*.example.com"); null for any host
 * @param matchPath          Path match pattern (e.g. "/api/*", "/health"); null for any path
 * @param matchHeaders       Header match criteria (key -> value); null or empty for no header matching
 * @param backendClusterRef  Reference to the target backend cluster by name
 * @param priority           Rule evaluation priority (lower value = higher priority, must be >= 0)
 * @param retryPolicyRef     Optional reference to a {@link RetryPolicyConfig} by name
 * @param timeoutMs          Request timeout in milliseconds (must be >= 0; 0 means use default)
 */
public record RouteConfig(
        @JsonProperty("name") String name,
        @JsonProperty("matchHost") String matchHost,
        @JsonProperty("matchPath") String matchPath,
        @JsonProperty("matchHeaders") Map<String, String> matchHeaders,
        @JsonProperty("backendClusterRef") String backendClusterRef,
        @JsonProperty("priority") int priority,
        @JsonProperty("retryPolicyRef") String retryPolicyRef,
        @JsonProperty("timeoutMs") long timeoutMs
) {

    public RouteConfig {
        // Defensive copy of matchHeaders
        matchHeaders = matchHeaders == null ? Map.of() : Map.copyOf(matchHeaders);
    }

    /**
     * Validate all fields for correctness.
     *
     * @throws IllegalArgumentException if any field is invalid
     */
    public void validate() {
        Objects.requireNonNull(name, "name");
        if (name.isBlank()) {
            throw new IllegalArgumentException("name must not be blank");
        }
        // At least one match criterion must be specified
        boolean hasHost = matchHost != null && !matchHost.isBlank();
        boolean hasPath = matchPath != null && !matchPath.isBlank();
        boolean hasHeaders = !matchHeaders.isEmpty();
        if (!hasHost && !hasPath && !hasHeaders) {
            throw new IllegalArgumentException("At least one match criterion (matchHost, matchPath, or matchHeaders) must be specified");
        }
        if (hasPath && !matchPath.startsWith("/")) {
            throw new IllegalArgumentException("matchPath must start with '/', got: " + matchPath);
        }
        Objects.requireNonNull(backendClusterRef, "backendClusterRef");
        if (backendClusterRef.isBlank()) {
            throw new IllegalArgumentException("backendClusterRef must not be blank");
        }
        if (priority < 0) {
            throw new IllegalArgumentException("priority must be >= 0, got: " + priority);
        }
        if (timeoutMs < 0) {
            throw new IllegalArgumentException("timeoutMs must be >= 0, got: " + timeoutMs);
        }
    }
}
