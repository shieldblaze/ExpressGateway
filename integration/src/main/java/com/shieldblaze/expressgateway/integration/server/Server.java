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
package com.shieldblaze.expressgateway.integration.server;

import com.shieldblaze.expressgateway.integration.event.ServerRestartEvent;
import com.shieldblaze.expressgateway.integration.event.ServerDestroyEvent;

import java.net.Inet4Address;
import java.net.Inet6Address;

/**
 * This class holds Server details such as id, address, etc.
 */
public interface Server {

    /**
     * Unique ID of this server
     */
    String id();

    /**
     * Server start time
     */
    long startTime();

    /**
     * Returns {@code true} if the server contains at least
     * 1 (one) connection else {@code false}
     */
    boolean inUse();

    /**
     * IPv4 Address of this server
     */
    Inet4Address ipv4Address();

    /**
     * IPv6 Address of this server
     */
    Inet6Address ipv6Address();

    /**
     * Restart this server
     */
    ServerRestartEvent<?> restart();

    /**
     * Shutdown this server
     */
    ServerDestroyEvent<?> destroy();

    /**
     * Provider of this Server
     */
    String providerName();
}
