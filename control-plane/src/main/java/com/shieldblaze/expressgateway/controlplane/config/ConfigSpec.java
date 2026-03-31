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
package com.shieldblaze.expressgateway.controlplane.config;

import com.fasterxml.jackson.annotation.JsonSubTypes;
import com.fasterxml.jackson.annotation.JsonTypeInfo;
import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.HealthCheckSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.ListenerSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.RateLimitSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.RoutingRuleSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.SecurityPolicySpec;
import com.shieldblaze.expressgateway.controlplane.config.types.TlsCertSpec;
import com.shieldblaze.expressgateway.controlplane.config.types.TransportSpec;

/**
 * Marker interface for all configuration spec payloads.
 * Each config domain (cluster, listener, routing rule, etc.) implements this interface
 * to define its own typed configuration structure.
 *
 * <p>Implementations must be serializable via Jackson and should use records
 * with {@code @JsonProperty} annotations for clean JSON mapping.</p>
 *
 * <p>HIGH-5 security fix: Uses {@code Id.NAME} with closed {@code @JsonSubTypes}
 * enumeration instead of {@code Id.CLASS} to prevent arbitrary class instantiation
 * via Jackson polymorphic deserialization (gadget attack vector).</p>
 */
@JsonTypeInfo(use = JsonTypeInfo.Id.NAME, property = "@type")
@JsonSubTypes({
        @JsonSubTypes.Type(value = ClusterSpec.class, name = "cluster"),
        @JsonSubTypes.Type(value = HealthCheckSpec.class, name = "healthCheck"),
        @JsonSubTypes.Type(value = ListenerSpec.class, name = "listener"),
        @JsonSubTypes.Type(value = RateLimitSpec.class, name = "rateLimit"),
        @JsonSubTypes.Type(value = RoutingRuleSpec.class, name = "routingRule"),
        @JsonSubTypes.Type(value = SecurityPolicySpec.class, name = "securityPolicy"),
        @JsonSubTypes.Type(value = TlsCertSpec.class, name = "tlsCert"),
        @JsonSubTypes.Type(value = TransportSpec.class, name = "transport")
})
public interface ConfigSpec {

    /**
     * Validate this spec's fields for correctness.
     *
     * @throws IllegalArgumentException if any field is invalid
     */
    void validate();
}
