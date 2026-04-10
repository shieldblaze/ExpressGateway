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

/**
 * Configuration for service discovery in clustered mode.
 */
@Value
@Builder
@JsonDeserialize(builder = ServiceDiscoveryConfig.ServiceDiscoveryConfigBuilder.class)
public class ServiceDiscoveryConfig {

    @JsonProperty("uri")
    String uri;

    @JsonProperty("trustAllCerts")
    boolean trustAllCerts;

    @Builder.Default
    @JsonProperty("hostnameVerification")
    boolean hostnameVerification = true;

    @JsonProperty("tls")
    TlsStoreConfig tls;

    @JsonPOJOBuilder(withPrefix = "")
    public static class ServiceDiscoveryConfigBuilder {
    }
}
