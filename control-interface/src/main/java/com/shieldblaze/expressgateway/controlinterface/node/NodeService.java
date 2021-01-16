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
package com.shieldblaze.expressgateway.controlinterface.node;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.LoadBalancerRegistry;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l4.TCPHealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l4.UDPHealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l7.HTTPHealthCheck;
import io.grpc.stub.StreamObserver;

import java.net.InetSocketAddress;
import java.net.URI;
import java.time.Duration;
import java.util.Map;

public class NodeService extends NodeServiceGrpc.NodeServiceImplBase {

    @Override
    public void add(NodeOuterClass.Node request, StreamObserver<NodeOuterClass.Response> responseObserver) {
        NodeOuterClass.Response response;

        try {
            L4LoadBalancer l4LoadBalancer = getLoadBalancerByID(request.getLoadBalancerID());

            HealthCheck healthCheck = null;

            // If 2 types of HealthCheck are present, then throw error.
            if (request.hasHealthCheckL4() && request.hasHealthCheckHttp()) {
                throw new IllegalArgumentException("2 types of HealthCheck defined. Expected only 1");
            }

            if (request.hasHealthCheckL4()) {
                NodeOuterClass.HealthCheckL4 healthCheckL4 = request.getHealthCheckL4();
                int samples = 100;
                if (healthCheckL4.getSamples() != 0) {
                    samples = healthCheckL4.getSamples();
                }

                if (request.getHealthCheckL4().getProtocol().equalsIgnoreCase("tcp")) {
                    healthCheck = new TCPHealthCheck(new InetSocketAddress(healthCheckL4.getAddress(), healthCheckL4.getPort()),
                            Duration.ofMillis(healthCheckL4.getTimeout()), samples);
                } else if (request.getHealthCheckL4().getProtocol().equalsIgnoreCase("udp")) {
                    healthCheck = new UDPHealthCheck(new InetSocketAddress(healthCheckL4.getAddress(), healthCheckL4.getPort()),
                            Duration.ofMillis(healthCheckL4.getTimeout()), samples);
                } else {
                    throw new IllegalArgumentException("Unknown HealthCheck L4 Protocol: " + request.getHealthCheckL4().getProtocol());
                }
            }

            if (request.hasHealthCheckHttp()) {
                NodeOuterClass.HealthCheckHTTP healthCheckHttp = request.getHealthCheckHttp();
                int samples = 100;
                if (healthCheckHttp.getSamples() != 0) {
                    samples = healthCheckHttp.getSamples();
                }

                healthCheck = new HTTPHealthCheck(URI.create(healthCheckHttp.getUri()), Duration.ofMillis(healthCheckHttp.getTimeout()),
                        samples, healthCheckHttp.getEnableTLSValidation());
            }

            Node node = new Node(l4LoadBalancer.cluster(), new InetSocketAddress(request.getAddress(), request.getPort()), request.getMaxConnections(), healthCheck);

            response = NodeOuterClass.Response.newBuilder()
                    .setResponseCode(1)
                    .setResponseText(node.id())
                    .build();
        } catch (Exception ex) {
            response = NodeOuterClass.Response.newBuilder()
                    .setResponseCode(1)
                    .setResponseText(ex.getLocalizedMessage())
                    .build();
        }

        responseObserver.onNext(response);
        responseObserver.onCompleted();
    }

    @Override
    public void get(NodeOuterClass.Node request, StreamObserver<NodeOuterClass.Response> responseObserver) {
        super.get(request, responseObserver);
    }

    @Override
    public void delete(NodeOuterClass.Node request, StreamObserver<NodeOuterClass.Response> responseObserver) {
        super.delete(request, responseObserver);
    }

    private L4LoadBalancer getLoadBalancerByID(String id) {
        for (Map.Entry<L4LoadBalancer, L4FrontListenerStartupEvent> entry : LoadBalancerRegistry.registry.entrySet()) {
            if (entry.getKey().ID.equalsIgnoreCase(id)) {
                return entry.getKey();
            }
        }

        throw new NullPointerException("No LoadBalancer found with ID: " + id);
    }
}
