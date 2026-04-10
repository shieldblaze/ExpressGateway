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

import java.util.ArrayList;
import java.util.List;

/**
 * Root configuration object for ShieldBlaze ExpressGateway.
 *
 * <p>This is an immutable value object constructed via the builder pattern.
 * After construction, call {@link #validate()} to verify all invariants.
 * Validation collects ALL violations rather than failing on the first,
 * so operators can fix every issue in one pass.</p>
 *
 * <p>Configuration is loaded by {@link ConfigLoader} with layered resolution:
 * defaults -> config file -> environment variables -> system properties.</p>
 */
@Value
@Builder
@JsonDeserialize(builder = GatewayConfig.GatewayConfigBuilder.class)
public class GatewayConfig {

    @JsonProperty("clusterId")
    String clusterId;

    @JsonProperty("runningMode")
    RunningMode runningMode;

    @JsonProperty("environment")
    Environment environment;

    @JsonProperty("restApi")
    RestApiConfig restApi;

    @JsonProperty("coordination")
    CoordinationConfig coordination;

    @JsonProperty("serviceDiscovery")
    ServiceDiscoveryConfig serviceDiscovery;

    @JsonProperty("loadBalancerTls")
    TlsStoreConfig loadBalancerTls;

    @JsonProperty("controlPlane")
    ControlPlaneConfig controlPlane;

    /**
     * Validates the entire configuration tree, collecting all violations.
     *
     * @throws ConfigValidationException if any validation violations are found
     */
    public void validate() {
        List<String> violations = new ArrayList<>();

        if (clusterId == null || clusterId.isBlank()) {
            violations.add("clusterId is required and must not be blank");
        }
        if (environment == null) {
            violations.add("environment is required");
        }
        if (runningMode == null) {
            violations.add("runningMode is required");
        }

        if (runningMode == RunningMode.CLUSTERED) {
            if (coordination == null) {
                violations.add("coordination is required in CLUSTERED mode");
            }
            if (serviceDiscovery == null) {
                violations.add("serviceDiscovery is required in CLUSTERED mode");
            }
        }

        // Validate nested configs (always validate if present, regardless of mode)
        if (restApi != null) {
            restApi.validate(violations);
        }
        if (coordination != null) {
            coordination.validate(violations);
        }
        if (controlPlane != null && controlPlane.isEnabled()) {
            controlPlane.validate(violations);
        }

        if (!violations.isEmpty()) {
            throw new ConfigValidationException(violations);
        }
    }

    @JsonPOJOBuilder(withPrefix = "")
    public static class GatewayConfigBuilder {
    }
}
