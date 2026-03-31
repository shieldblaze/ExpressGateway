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
 * Configuration spec for rate limiting.
 *
 * @param name              The rate limit policy name
 * @param requestsPerSecond Maximum sustained request rate
 * @param burstSize         Maximum burst size above the sustained rate
 * @param scope             Rate limit scope ("global", "per-ip", "per-header")
 * @param headerName        Header name to key on when scope is "per-header" (optional otherwise)
 */
public record RateLimitSpec(
        @JsonProperty("name") String name,
        @JsonProperty("requestsPerSecond") long requestsPerSecond,
        @JsonProperty("burstSize") long burstSize,
        @JsonProperty("scope") String scope,
        @JsonProperty("headerName") String headerName
) implements ConfigSpec {

    private static final Set<String> VALID_SCOPES = Set.of("global", "per-ip", "per-header");

    @Override
    public void validate() {
        Objects.requireNonNull(name, "name");
        if (name.isBlank()) {
            throw new IllegalArgumentException("name must not be blank");
        }
        if (requestsPerSecond < 1) {
            throw new IllegalArgumentException("requestsPerSecond must be >= 1, got: " + requestsPerSecond);
        }
        if (burstSize < 1) {
            throw new IllegalArgumentException("burstSize must be >= 1, got: " + burstSize);
        }
        if (burstSize < requestsPerSecond) {
            throw new IllegalArgumentException(
                    "burstSize (" + burstSize + ") must be >= requestsPerSecond (" + requestsPerSecond + ")");
        }
        Objects.requireNonNull(scope, "scope");
        if (!VALID_SCOPES.contains(scope)) {
            throw new IllegalArgumentException("scope must be one of " + VALID_SCOPES + ", got: " + scope);
        }
        if ("per-header".equals(scope)) {
            if (headerName == null || headerName.isBlank()) {
                throw new IllegalArgumentException("headerName is required when scope is 'per-header'");
            }
        }
    }
}
