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

/**
 * Configuration schema for traffic shaping (bandwidth limiting).
 *
 * <p>Based on Netty's {@code GlobalTrafficShapingHandler} and
 * {@code ChannelTrafficShapingHandler} concepts. Limits can be applied
 * globally across all connections or per-connection.</p>
 *
 * @param name                     The traffic shaping policy name
 * @param maxBandwidthBytesPerSec  Global maximum bandwidth in bytes/sec (must be >= 0; 0 means unlimited)
 * @param burstBytes               Burst allowance in bytes (must be >= 0)
 * @param perConnectionLimitBytesPerSec Per-connection bandwidth limit in bytes/sec (must be >= 0; 0 means unlimited)
 * @param readLimitBytesPerSec     Global read bandwidth limit in bytes/sec (must be >= 0; 0 means unlimited)
 * @param writeLimitBytesPerSec    Global write bandwidth limit in bytes/sec (must be >= 0; 0 means unlimited)
 */
public record TrafficShapingConfig(
        @JsonProperty("name") String name,
        @JsonProperty("maxBandwidthBytesPerSec") long maxBandwidthBytesPerSec,
        @JsonProperty("burstBytes") long burstBytes,
        @JsonProperty("perConnectionLimitBytesPerSec") long perConnectionLimitBytesPerSec,
        @JsonProperty("readLimitBytesPerSec") long readLimitBytesPerSec,
        @JsonProperty("writeLimitBytesPerSec") long writeLimitBytesPerSec
) {

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
        if (maxBandwidthBytesPerSec < 0) {
            throw new IllegalArgumentException("maxBandwidthBytesPerSec must be >= 0, got: " + maxBandwidthBytesPerSec);
        }
        if (burstBytes < 0) {
            throw new IllegalArgumentException("burstBytes must be >= 0, got: " + burstBytes);
        }
        if (perConnectionLimitBytesPerSec < 0) {
            throw new IllegalArgumentException("perConnectionLimitBytesPerSec must be >= 0, got: " + perConnectionLimitBytesPerSec);
        }
        if (readLimitBytesPerSec < 0) {
            throw new IllegalArgumentException("readLimitBytesPerSec must be >= 0, got: " + readLimitBytesPerSec);
        }
        if (writeLimitBytesPerSec < 0) {
            throw new IllegalArgumentException("writeLimitBytesPerSec must be >= 0, got: " + writeLimitBytesPerSec);
        }
        // If both global and per-connection limits are set, per-connection must not exceed global
        if (maxBandwidthBytesPerSec > 0 && perConnectionLimitBytesPerSec > maxBandwidthBytesPerSec) {
            throw new IllegalArgumentException(
                    "perConnectionLimitBytesPerSec (" + perConnectionLimitBytesPerSec +
                    ") must not exceed maxBandwidthBytesPerSec (" + maxBandwidthBytesPerSec + ")");
        }
    }
}
