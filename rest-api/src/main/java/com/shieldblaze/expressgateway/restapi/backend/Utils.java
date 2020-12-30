/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.restapi.backend;

import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l4.TCPHealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l4.UDPHealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l7.HTTPHealthCheck;

import java.net.InetSocketAddress;
import java.net.URI;
import java.security.KeyManagementException;
import java.security.NoSuchAlgorithmException;
import java.time.Duration;

public class Utils {

    static HealthCheck determine(AddNodeHandler addNodeHandler) throws KeyManagementException, NoSuchAlgorithmException {
        if (addNodeHandler.healthCheckContext().type().equalsIgnoreCase("tcp")) {
            InetSocketAddress socketAddress = new InetSocketAddress(addNodeHandler.healthCheckContext().host(), addNodeHandler.port());
            return new TCPHealthCheck(socketAddress, Duration.ofSeconds(addNodeHandler.healthCheckContext().timeout()));
        } else if (addNodeHandler.healthCheckContext().type().equalsIgnoreCase("udp")) {
            InetSocketAddress socketAddress = new InetSocketAddress(addNodeHandler.healthCheckContext().host(), addNodeHandler.port());
            return new UDPHealthCheck(socketAddress, Duration.ofSeconds(addNodeHandler.healthCheckContext().timeout()));
        } else if (addNodeHandler.healthCheckContext().type().equalsIgnoreCase("http")) {
            AddNodeHandler.HealthCheckContext ctx = addNodeHandler.healthCheckContext();
            URI uri = URI.create("http://" + ctx.host() + ":" + ctx.port() + "/" + ctx.httpPath());
            return new HTTPHealthCheck(uri, Duration.ofSeconds(addNodeHandler.healthCheckContext().timeout()), false);
        } else if (addNodeHandler.healthCheckContext().type().equalsIgnoreCase("https")) {
            AddNodeHandler.HealthCheckContext ctx = addNodeHandler.healthCheckContext();
            URI uri = URI.create("httsp://" + ctx.host() + ":" + ctx.port() + "/" + ctx.httpPath());
            return new HTTPHealthCheck(uri, Duration.ofSeconds(addNodeHandler.healthCheckContext().timeout()), ctx.enableTLSValidation());
        }

        return null;
    }
}
