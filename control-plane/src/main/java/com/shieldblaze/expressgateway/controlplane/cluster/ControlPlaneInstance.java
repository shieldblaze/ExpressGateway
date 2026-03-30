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
package com.shieldblaze.expressgateway.controlplane.cluster;

import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Objects;

/**
 * Metadata about a single control plane instance registered in the cluster.
 *
 * <p>Each instance registers itself in the KV store under
 * {@code /expressgateway/controlplane/instances/{instanceId}} with a JSON-serialized
 * representation of this record. Other instances discover peers by watching that prefix.</p>
 *
 * @param instanceId    unique identifier for this CP instance (typically a UUID)
 * @param region        geographic region where this instance is deployed (e.g. "us-east-1")
 * @param grpcAddress   the host/IP address that data-plane nodes use to connect to this instance
 * @param grpcPort      the gRPC port this instance listens on
 * @param registeredAt  epoch millis when this instance first registered
 * @param lastHeartbeat epoch millis of the most recent heartbeat update
 */
public record ControlPlaneInstance(
        @JsonProperty("instanceId") String instanceId,
        @JsonProperty("region") String region,
        @JsonProperty("grpcAddress") String grpcAddress,
        @JsonProperty("grpcPort") int grpcPort,
        @JsonProperty("registeredAt") long registeredAt,
        @JsonProperty("lastHeartbeat") long lastHeartbeat
) {

    public ControlPlaneInstance {
        Objects.requireNonNull(instanceId, "instanceId");
        Objects.requireNonNull(region, "region");
        Objects.requireNonNull(grpcAddress, "grpcAddress");
        if (instanceId.isBlank()) {
            throw new IllegalArgumentException("instanceId must not be blank");
        }
        if (region.isBlank()) {
            throw new IllegalArgumentException("region must not be blank");
        }
        if (grpcAddress.isBlank()) {
            throw new IllegalArgumentException("grpcAddress must not be blank");
        }
        if (grpcPort < 1 || grpcPort > 65535) {
            throw new IllegalArgumentException("grpcPort must be in range [1, 65535], got: " + grpcPort);
        }
        if (registeredAt < 0) {
            throw new IllegalArgumentException("registeredAt must be >= 0, got: " + registeredAt);
        }
        if (lastHeartbeat < 0) {
            throw new IllegalArgumentException("lastHeartbeat must be >= 0, got: " + lastHeartbeat);
        }
    }

    /**
     * Returns a new instance with the {@code lastHeartbeat} field updated.
     *
     * @param newHeartbeat the new heartbeat epoch millis
     * @return a copy of this record with the updated heartbeat timestamp
     */
    public ControlPlaneInstance withHeartbeat(long newHeartbeat) {
        return new ControlPlaneInstance(instanceId, region, grpcAddress, grpcPort, registeredAt, newHeartbeat);
    }
}
