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
 * Configuration spec for Netty transport layer settings.
 *
 * @param name              The transport configuration name
 * @param transportType     The Netty transport type ("nio", "epoll", "io_uring")
 * @param receiveBufferSize Socket receive buffer size in bytes (SO_RCVBUF)
 * @param sendBufferSize    Socket send buffer size in bytes (SO_SNDBUF)
 * @param tcpFastOpen       Whether to enable TCP Fast Open (RFC 7413)
 * @param proxyProtocol     Whether to enable PROXY protocol (v1/v2) on incoming connections
 */
public record TransportSpec(
        @JsonProperty("name") String name,
        @JsonProperty("transportType") String transportType,
        @JsonProperty("receiveBufferSize") int receiveBufferSize,
        @JsonProperty("sendBufferSize") int sendBufferSize,
        @JsonProperty("tcpFastOpen") boolean tcpFastOpen,
        @JsonProperty("proxyProtocol") boolean proxyProtocol
) implements ConfigSpec {

    private static final Set<String> VALID_TRANSPORT_TYPES = Set.of("nio", "epoll", "io_uring");

    @Override
    public void validate() {
        Objects.requireNonNull(name, "name");
        if (name.isBlank()) {
            throw new IllegalArgumentException("name must not be blank");
        }
        Objects.requireNonNull(transportType, "transportType");
        if (!VALID_TRANSPORT_TYPES.contains(transportType)) {
            throw new IllegalArgumentException(
                    "transportType must be one of " + VALID_TRANSPORT_TYPES + ", got: " + transportType);
        }
        if (receiveBufferSize < 1) {
            throw new IllegalArgumentException("receiveBufferSize must be >= 1, got: " + receiveBufferSize);
        }
        if (sendBufferSize < 1) {
            throw new IllegalArgumentException("sendBufferSize must be >= 1, got: " + sendBufferSize);
        }
    }
}
