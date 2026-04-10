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
 * Configuration for the management REST API endpoint.
 */
@Value
@Builder
@JsonDeserialize(builder = RestApiConfig.RestApiConfigBuilder.class)
public class RestApiConfig {

    @Builder.Default
    @JsonProperty("bindAddress")
    String bindAddress = "0.0.0.0";

    @Builder.Default
    @JsonProperty("port")
    int port = 9110;

    @JsonProperty("tlsEnabled")
    boolean tlsEnabled;

    @JsonProperty("tls")
    TlsStoreConfig tls;

    /**
     * Validates this REST API configuration.
     *
     * @param violations mutable list to collect violations into
     */
    public void validate(List<String> violations) {
        if (port < 1 || port > 65535) {
            violations.add("restApi.port must be between 1 and 65535, got: " + port);
        }
        if (bindAddress == null || bindAddress.isBlank()) {
            violations.add("restApi.bindAddress must not be blank");
        }
        if (tlsEnabled && tls == null) {
            violations.add("restApi.tls must be configured when tlsEnabled is true");
        }
    }

    @JsonPOJOBuilder(withPrefix = "")
    public static class RestApiConfigBuilder {
    }
}
