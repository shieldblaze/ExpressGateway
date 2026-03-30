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
package com.shieldblaze.expressgateway.controlplane.config.types;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;

import java.util.Objects;
import java.util.Set;

/**
 * Configuration spec for a network listener (frontend bind point).
 *
 * @param name        The listener name
 * @param bindAddress The address to bind to (e.g. "0.0.0.0", "::1")
 * @param bindPort    The port to bind to
 * @param protocol    The protocol type ("tcp", "http", "http2", "http3", "udp")
 * @param tlsCertName Optional reference to a {@link TlsCertSpec} by name (null if no TLS)
 * @param clusterName Reference to the backend {@link ClusterSpec} by name
 */
public record ListenerSpec(
        @JsonProperty("name") String name,
        @JsonProperty("bindAddress") String bindAddress,
        @JsonProperty("bindPort") int bindPort,
        @JsonProperty("protocol") String protocol,
        @JsonProperty("tlsCertName") String tlsCertName,
        @JsonProperty("clusterName") String clusterName
) implements ConfigSpec {

    private static final Set<String> VALID_PROTOCOLS = Set.of("tcp", "http", "http2", "http3", "udp");

    @Override
    public void validate() {
        Objects.requireNonNull(name, "name");
        if (name.isBlank()) {
            throw new IllegalArgumentException("name must not be blank");
        }
        Objects.requireNonNull(bindAddress, "bindAddress");
        if (bindAddress.isBlank()) {
            throw new IllegalArgumentException("bindAddress must not be blank");
        }
        if (bindPort < 1 || bindPort > 65535) {
            throw new IllegalArgumentException("bindPort must be in range [1, 65535], got: " + bindPort);
        }
        Objects.requireNonNull(protocol, "protocol");
        if (!VALID_PROTOCOLS.contains(protocol)) {
            throw new IllegalArgumentException(
                    "protocol must be one of " + VALID_PROTOCOLS + ", got: " + protocol);
        }
        // tlsCertName is optional (null allowed for non-TLS listeners)
        // HTTP/3 requires TLS per RFC 9114 Section 3.3
        if ("http3".equals(protocol) && (tlsCertName == null || tlsCertName.isBlank())) {
            throw new IllegalArgumentException("http3 protocol requires a tlsCertName (TLS is mandatory per RFC 9114)");
        }
        Objects.requireNonNull(clusterName, "clusterName");
        if (clusterName.isBlank()) {
            throw new IllegalArgumentException("clusterName must not be blank");
        }
    }
}
