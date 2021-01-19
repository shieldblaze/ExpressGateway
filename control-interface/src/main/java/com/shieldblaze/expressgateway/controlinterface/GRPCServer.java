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
package com.shieldblaze.expressgateway.controlinterface;

import com.shieldblaze.expressgateway.controlinterface.configuration.BufferService;
import com.shieldblaze.expressgateway.controlinterface.configuration.EventLoopService;
import com.shieldblaze.expressgateway.controlinterface.configuration.EventStreamService;
import com.shieldblaze.expressgateway.controlinterface.configuration.HTTPService;
import com.shieldblaze.expressgateway.controlinterface.configuration.HealthCheckService;
import com.shieldblaze.expressgateway.controlinterface.configuration.TransportService;
import com.shieldblaze.expressgateway.controlinterface.loadbalancer.TCPLoadBalancerService;
import com.shieldblaze.expressgateway.controlinterface.loadbalancer.UDPLoadBalancerService;
import com.shieldblaze.expressgateway.controlinterface.node.NodeService;
import io.grpc.Server;
import io.grpc.ServerBuilder;

public class GRPCServer {

    public static void main(String[] args) throws Exception {
        // Create a new server to listen on port 8080
        Server server = ServerBuilder.forPort(8080)
                .addService(new BufferService())
                .addService(new EventLoopService())
                .addService(new EventStreamService())
                .addService(new HealthCheckService())
                .addService(new HTTPService())
                .addService(new TransportService())

                .addService(new TCPLoadBalancerService())
                .addService(new UDPLoadBalancerService())

                .addService(new NodeService())
                .build();

        server.start();

        System.out.println("Server started");

        server.awaitTermination();
    }
}
