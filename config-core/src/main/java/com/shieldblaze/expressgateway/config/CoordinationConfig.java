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
 * Configuration for the coordination backend used in clustered mode.
 * Supports ZooKeeper, etcd, and Consul.
 */
@Value
@Builder
@JsonDeserialize(builder = CoordinationConfig.CoordinationConfigBuilder.class)
public class CoordinationConfig {

    @JsonProperty("backend")
    CoordinationBackend backend;

    @JsonProperty("endpoints")
    List<String> endpoints;

    @Builder.Default
    @JsonProperty("sessionTimeoutMs")
    int sessionTimeoutMs = 30000;

    @Builder.Default
    @JsonProperty("connectionTimeoutMs")
    int connectionTimeoutMs = 15000;

    @JsonProperty("tls")
    TlsStoreConfig tls;

    @JsonProperty("username")
    String username;

    @JsonProperty("password")
    char[] password;

    @JsonProperty("namespace")
    String namespace;

    /**
     * Validates this coordination configuration.
     *
     * @param violations mutable list to collect violations into
     */
    public void validate(List<String> violations) {
        if (backend == null) {
            violations.add("coordination.backend is required");
        }
        if (endpoints == null || endpoints.isEmpty()) {
            violations.add("coordination.endpoints must contain at least one endpoint");
        } else {
            for (int i = 0; i < endpoints.size(); i++) {
                if (endpoints.get(i) == null || endpoints.get(i).isBlank()) {
                    violations.add("coordination.endpoints[" + i + "] must not be blank");
                }
            }
        }
        if (sessionTimeoutMs <= 0) {
            violations.add("coordination.sessionTimeoutMs must be positive, got: " + sessionTimeoutMs);
        }
        if (connectionTimeoutMs <= 0) {
            violations.add("coordination.connectionTimeoutMs must be positive, got: " + connectionTimeoutMs);
        }
    }

    @JsonPOJOBuilder(withPrefix = "")
    public static class CoordinationConfigBuilder {
    }
}
