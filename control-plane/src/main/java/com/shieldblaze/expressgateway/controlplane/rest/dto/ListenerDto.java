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

import com.shieldblaze.expressgateway.controlplane.config.types.ListenerSpec;

/**
 * DTO for listener configuration CRUD operations.
 *
 * @param name        the listener name
 * @param bindAddress the address to bind to (e.g. "0.0.0.0", "::1")
 * @param bindPort    the port to bind to
 * @param protocol    the protocol type ("tcp", "http", "http2", "http3", "udp")
 * @param tlsCertName optional reference to a TLS certificate by name
 * @param clusterName reference to the backend cluster by name
 */
public record ListenerDto(
        String name,
        String bindAddress,
        int bindPort,
        String protocol,
        String tlsCertName,
        String clusterName
) {

    /**
     * Convert this DTO to a {@link ListenerSpec}.
     */
    public ListenerSpec toSpec() {
        return new ListenerSpec(name, bindAddress, bindPort, protocol, tlsCertName, clusterName);
    }

    /**
     * Create a DTO from a {@link ListenerSpec}.
     */
    public static ListenerDto from(ListenerSpec spec) {
        return new ListenerDto(
                spec.name(),
                spec.bindAddress(),
                spec.bindPort(),
                spec.protocol(),
                spec.tlsCertName(),
                spec.clusterName()
        );
    }
}
