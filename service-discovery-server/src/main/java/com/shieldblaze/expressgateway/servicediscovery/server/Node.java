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
package com.shieldblaze.expressgateway.servicediscovery.server;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.fasterxml.jackson.annotation.JsonRootName;

import java.net.InetAddress;
import java.net.InetSocketAddress;

import static com.shieldblaze.expressgateway.common.utils.StringUtil.validateNotNullOrEmpty;

@JsonRootName("ExpressGatewayNode")
public class Node {

    @JsonProperty("ID")
    private String id;

    @JsonProperty("IPAddress")
    private String ipAddress;

    @JsonProperty("Port")
    private int port;

    @JsonProperty("TLSEnabled")
    private boolean tlsEnabled;

    public Node() {
        this(null, null, -1, false);
    }

    public Node(String id, String ipAddress, int port, boolean tlsEnabled) {
        this.id = id;
        this.ipAddress = ipAddress;
        this.port = port;
        this.tlsEnabled = tlsEnabled;
    }

    public String id() {
        return id;
    }

    public String ipAddress() {
        return ipAddress;
    }

    public int port() {
        return port;
    }

    public boolean tlsEnabled() {
        return tlsEnabled;
    }

    /**
     * Validate ID, IP address and Port.
     *
     * @throws IllegalArgumentException If something is invalid
     * @throws NullPointerException     If ID is null
     */
    public void validate() {
        validateNotNullOrEmpty(id, "ID");

        try {
            InetAddress.getByName(ipAddress);
        } catch (Exception ex) {
            throw new IllegalArgumentException(ex);
        }

        try {
            new InetSocketAddress(ipAddress, port);
        } catch (Exception ex) {
            throw new IllegalArgumentException(ex);
        }
    }

    @Override
    public String toString() {
        return "ExpressGatewayNode{" +
                "ipAddress='" + ipAddress + '\'' +
                ", port=" + port +
                ", tlsEnabled=" + tlsEnabled +
                '}';
    }
}
