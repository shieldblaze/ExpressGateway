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
package com.shieldblaze.expressgateway.config;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.fasterxml.jackson.databind.annotation.JsonDeserialize;
import com.fasterxml.jackson.databind.annotation.JsonPOJOBuilder;
import lombok.Builder;
import lombok.Value;

import java.util.List;

/**
 * Configuration for the control plane (gRPC server for cluster coordination).
 *
 * <p>When {@code enabled} is false, this configuration block is ignored entirely.
 * When acting as an agent (data plane node), {@code controlPlaneAddress} and
 * {@code controlPlanePort} point to the remote control plane leader.</p>
 */
@Value
@Builder
@JsonDeserialize(builder = ControlPlaneConfig.ControlPlaneConfigBuilder.class)
public class ControlPlaneConfig {

    @JsonProperty("enabled")
    boolean enabled;

    @Builder.Default
    @JsonProperty("grpcBindAddress")
    String grpcBindAddress = "0.0.0.0";

    @Builder.Default
    @JsonProperty("grpcPort")
    int grpcPort = 9443;

    @Builder.Default
    @JsonProperty("heartbeatIntervalMs")
    long heartbeatIntervalMs = 10000;

    @Builder.Default
    @JsonProperty("heartbeatMissThreshold")
    int heartbeatMissThreshold = 3;

    @Builder.Default
    @JsonProperty("maxNodes")
    int maxNodes = 100;

    @JsonProperty("controlPlaneAddress")
    String controlPlaneAddress;

    @JsonProperty("controlPlanePort")
    int controlPlanePort;

    /**
     * Validates this control plane configuration.
     *
     * @param violations mutable list to collect violations into
     */
    public void validate(List<String> violations) {
        if (!enabled) {
            return; // nothing to validate when disabled
        }
        if (grpcPort < 1 || grpcPort > 65535) {
            violations.add("controlPlane.grpcPort must be between 1 and 65535, got: " + grpcPort);
        }
        if (grpcBindAddress == null || grpcBindAddress.isBlank()) {
            violations.add("controlPlane.grpcBindAddress must not be blank");
        }
        if (heartbeatIntervalMs <= 0) {
            violations.add("controlPlane.heartbeatIntervalMs must be positive, got: " + heartbeatIntervalMs);
        }
        if (heartbeatMissThreshold <= 0) {
            violations.add("controlPlane.heartbeatMissThreshold must be positive, got: " + heartbeatMissThreshold);
        }
        if (maxNodes <= 0) {
            violations.add("controlPlane.maxNodes must be positive, got: " + maxNodes);
        }
        if (controlPlaneAddress != null && !controlPlaneAddress.isBlank()) {
            // Agent mode: validate remote control plane port
            if (controlPlanePort < 1 || controlPlanePort > 65535) {
                violations.add("controlPlane.controlPlanePort must be between 1 and 65535 when controlPlaneAddress is set, got: " + controlPlanePort);
            }
        }
    }

    @JsonPOJOBuilder(withPrefix = "")
    public static class ControlPlaneConfigBuilder {
    }
}
