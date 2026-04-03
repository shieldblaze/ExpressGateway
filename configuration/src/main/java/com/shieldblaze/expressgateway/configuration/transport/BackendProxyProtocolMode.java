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
package com.shieldblaze.expressgateway.configuration.transport;

/**
 * Controls whether the proxy sends a HAProxy PROXY protocol header
 * to backend servers on new connections.
 *
 * <ul>
 *   <li>{@link #OFF} - Do not send PROXY protocol to backends (default)</li>
 *   <li>{@link #V1} - Send PROXY protocol v1 (text-based) header</li>
 *   <li>{@link #V2} - Send PROXY protocol v2 (binary) header</li>
 * </ul>
 *
 * <p>Unlike {@link ProxyProtocolMode} (which controls <em>inbound</em> decoding and
 * includes {@code AUTO} for auto-detection), this enum controls <em>outbound</em>
 * encoding where the sender must choose a specific version.</p>
 */
public enum BackendProxyProtocolMode {

    /**
     * Do not send PROXY protocol header to backends (default)
     */
    OFF,

    /**
     * Send PROXY protocol v1 (text-based: {@code PROXY TCP4 srcIP dstIP srcPort dstPort\r\n})
     */
    V1,

    /**
     * Send PROXY protocol v2 (binary header with signature)
     */
    V2
}
