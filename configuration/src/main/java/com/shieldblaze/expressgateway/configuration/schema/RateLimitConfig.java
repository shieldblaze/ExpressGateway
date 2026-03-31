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

import java.util.Objects;
import java.util.Set;

/**
 * Configuration schema for rate limiting.
 *
 * @param name              The rate limit policy name
 * @param requestsPerSecond Maximum sustained request rate (must be >= 1)
 * @param burstSize         Maximum burst size above the sustained rate (must be >= requestsPerSecond)
 * @param keyExtractor      How to extract the rate-limit key ("IP", "HEADER", "PATH")
 * @param headerName        Header name to key on when keyExtractor is "HEADER" (optional otherwise)
 * @param responseStatusCode HTTP status code to return when rate limited (default: 429)
 * @param retryAfterSeconds  Value for Retry-After header in seconds (must be >= 0; 0 means no header)
 */
public record RateLimitConfig(
        @JsonProperty("name") String name,
        @JsonProperty("requestsPerSecond") long requestsPerSecond,
        @JsonProperty("burstSize") long burstSize,
        @JsonProperty("keyExtractor") String keyExtractor,
        @JsonProperty("headerName") String headerName,
        @JsonProperty("responseStatusCode") int responseStatusCode,
        @JsonProperty("retryAfterSeconds") int retryAfterSeconds
) {

    private static final Set<String> VALID_KEY_EXTRACTORS = Set.of("IP", "HEADER", "PATH");

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
        Objects.requireNonNull(keyExtractor, "keyExtractor");
        if (!VALID_KEY_EXTRACTORS.contains(keyExtractor)) {
            throw new IllegalArgumentException("keyExtractor must be one of " + VALID_KEY_EXTRACTORS + ", got: " + keyExtractor);
        }
        if ("HEADER".equals(keyExtractor)) {
            if (headerName == null || headerName.isBlank()) {
                throw new IllegalArgumentException("headerName is required when keyExtractor is 'HEADER'");
            }
        }
        if (responseStatusCode < 100 || responseStatusCode > 599) {
            throw new IllegalArgumentException("responseStatusCode must be in range [100, 599], got: " + responseStatusCode);
        }
        if (retryAfterSeconds < 0) {
            throw new IllegalArgumentException("retryAfterSeconds must be >= 0, got: " + retryAfterSeconds);
        }
    }
}
