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
package com.shieldblaze.expressgateway.controlplane.rest.dto;

import com.shieldblaze.expressgateway.controlplane.config.types.HealthCheckSpec;

/**
 * DTO for health check configuration CRUD operations.
 *
 * @param name               the health check name
 * @param type               the probe type ("tcp", "http", "udp")
 * @param intervalSeconds    interval between probes in seconds
 * @param timeoutSeconds     per-probe timeout in seconds
 * @param healthyThreshold   consecutive successes to mark healthy
 * @param unhealthyThreshold consecutive failures to mark unhealthy
 * @param httpPath           HTTP path for HTTP probes (optional)
 * @param expectedStatusCode expected HTTP status code for HTTP probes (optional)
 */
public record HealthCheckDto(
        String name,
        String type,
        int intervalSeconds,
        int timeoutSeconds,
        int healthyThreshold,
        int unhealthyThreshold,
        String httpPath,
        int expectedStatusCode
) {

    /**
     * Convert this DTO to a {@link HealthCheckSpec}.
     */
    public HealthCheckSpec toSpec() {
        return new HealthCheckSpec(name, type, intervalSeconds, timeoutSeconds,
                healthyThreshold, unhealthyThreshold, httpPath, expectedStatusCode);
    }

    /**
     * Create a DTO from a {@link HealthCheckSpec}.
     */
    public static HealthCheckDto from(HealthCheckSpec spec) {
        return new HealthCheckDto(
                spec.name(),
                spec.type(),
                spec.intervalSeconds(),
                spec.timeoutSeconds(),
                spec.healthyThreshold(),
                spec.unhealthyThreshold(),
                spec.httpPath(),
                spec.expectedStatusCode()
        );
    }
}
