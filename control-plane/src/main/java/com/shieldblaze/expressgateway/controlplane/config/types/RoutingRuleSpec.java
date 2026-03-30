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
package com.shieldblaze.expressgateway.controlplane.config.types;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;

import java.util.Objects;
import java.util.Set;

/**
 * Configuration spec for an L7 routing rule.
 *
 * @param name          The rule name
 * @param priority      Rule evaluation priority (lower value = higher priority)
 * @param matchType     The match criteria type ("host", "path", "header")
 * @param matchValue    The value to match against (e.g. "example.com", "/api/*")
 * @param targetCluster Reference to the target {@link ClusterSpec} by name
 */
public record RoutingRuleSpec(
        @JsonProperty("name") String name,
        @JsonProperty("priority") int priority,
        @JsonProperty("matchType") String matchType,
        @JsonProperty("matchValue") String matchValue,
        @JsonProperty("targetCluster") String targetCluster
) implements ConfigSpec {

    private static final Set<String> VALID_MATCH_TYPES = Set.of("host", "path", "header");

    @Override
    public void validate() {
        Objects.requireNonNull(name, "name");
        if (name.isBlank()) {
            throw new IllegalArgumentException("name must not be blank");
        }
        if (priority < 0) {
            throw new IllegalArgumentException("priority must be >= 0, got: " + priority);
        }
        Objects.requireNonNull(matchType, "matchType");
        if (!VALID_MATCH_TYPES.contains(matchType)) {
            throw new IllegalArgumentException(
                    "matchType must be one of " + VALID_MATCH_TYPES + ", got: " + matchType);
        }
        Objects.requireNonNull(matchValue, "matchValue");
        if (matchValue.isBlank()) {
            throw new IllegalArgumentException("matchValue must not be blank");
        }
        Objects.requireNonNull(targetCluster, "targetCluster");
        if (targetCluster.isBlank()) {
            throw new IllegalArgumentException("targetCluster must not be blank");
        }
    }
}
