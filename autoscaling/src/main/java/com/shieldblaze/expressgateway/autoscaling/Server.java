/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.autoscaling;

import com.shieldblaze.expressgateway.autoscaling.event.ServerRestartEvent;
import com.shieldblaze.expressgateway.autoscaling.event.ServerShutdownEvent;

import java.net.Inet4Address;
import java.net.Inet6Address;

/**
 * Server Interface
 */
public interface Server {

    /**
     * Name of the server
     */
    String name();

    /**
     * DNS Record of the server
     */
    DNSRecord dnsRecord();

    /**
     * Server start time
     */
    long startTime();

    /**
     * Returns {@code true} if the server is created by
     * autoscaling else {@code false}
     */
    boolean autoscaled();

    /**
     * Returns {@code true} if the server contains at least
     * 1 (one) connection else {@code false}
     */
    boolean inUse();

    /**
     * IPv4 Address of the server
     */
    Inet4Address ipv4Address();

    /**
     * IPv6 Address of the server
     */
    Inet6Address ipv6Address();

    /**
     * Restart the server
     */
    ServerRestartEvent restart();

    /**
     * Shutdown the server
     */
    ServerShutdownEvent shutdown();
}
