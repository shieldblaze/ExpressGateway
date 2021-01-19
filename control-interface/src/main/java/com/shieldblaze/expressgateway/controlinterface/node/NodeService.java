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
import com.shieldblaze.expressgateway.common.utils.Number;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.LoadBalancerProperty;
import com.shieldblaze.expressgateway.core.loadbalancer.LoadBalancerRegistry;
import com.shieldblaze.expressgateway.healthcheck.HealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l4.TCPHealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l4.UDPHealthCheck;
import com.shieldblaze.expressgateway.healthcheck.l7.HTTPHealthCheck;
import io.grpc.Status;
import io.grpc.stub.StreamObserver;

import java.net.InetSocketAddress;
import java.net.URI;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * <p> {@linkplain NodeService} can perform the following operations. </p>
 * <ul>
 *     <li> Add a node into Cluster </li>
 *     <li> Get a node from Cluster </li>
 *     <li> Get all nodes from Cluster </li>
 *     <li> Delete a node from Cluster </li>
 * </ul>
 */
public class NodeService extends NodeServiceGrpc.NodeServiceImplBase {

    @Override
    public void add(NodeOuterClass.AddRequest request, StreamObserver<NodeOuterClass.AddResponse> responseObserver) {
        NodeOuterClass.AddResponse response;

        try {
            L4LoadBalancer l4LoadBalancer = getLoadBalancerByID(request.getLoadBalancerID());
            HealthCheck healthCheck = null;

            if (request.hasHealthCheckL4() && request.hasHealthCheckHttp()) {
                throw new IllegalArgumentException("2 types of HealthCheck defined. Expected only 1");
            }

            // If HealthCheck L4 is present then parse it.
            if (request.hasHealthCheckL4()) {
                NodeOuterClass.HealthCheckL4 healthCheckL4 = request.getHealthCheckL4();
                int samples = 100; // Default value

                // If Samples count is not 0 then parse it.
                if (healthCheckL4.getSamples() != 0) {
                    samples = Number.checkPositive(healthCheckL4.getSamples(), "Samples");
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

            // If HealthCheck-HTTP is present then parse it.
            if (request.hasHealthCheckHttp()) {
                NodeOuterClass.HealthCheckHTTP healthCheckHttp = request.getHealthCheckHttp();
                int samples = 100; // Default value

                // If Samples count is not 0 then parse it.
                if (healthCheckHttp.getSamples() != 0) {
                    samples = healthCheckHttp.getSamples();
                }

                healthCheck = new HTTPHealthCheck(URI.create(healthCheckHttp.getUri()), Duration.ofMillis(healthCheckHttp.getTimeout()),
                        samples, healthCheckHttp.getEnableTLSValidation());
            }

            // Create a new Node under the specified Load Balancer
            Node node = new Node(l4LoadBalancer.cluster(), new InetSocketAddress(request.getAddress(), request.getPort()), request.getMaxConnections(), healthCheck);

            response = NodeOuterClass.AddResponse.newBuilder()
                    .setSuccess(true)
                    .setNodeId(node.id())
                    .build();

            responseObserver.onNext(response);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(Status.INVALID_ARGUMENT.augmentDescription(ex.getLocalizedMessage()).asRuntimeException());
        }
    }

    @Override
    public void get(NodeOuterClass.GetRequest request, StreamObserver<NodeOuterClass.GetResponse> responseObserver) {
        try {
            L4LoadBalancer l4LoadBalancer = getLoadBalancerByID(request.getLoadBalancerId());
            Node node = null;

            for (Node n : l4LoadBalancer.cluster().nodes()) {
                if (n.id().equalsIgnoreCase(request.getNodeId())) {
                    node = n;
                    break;
                }
            }

            // If node is null then it means we couldn't find the node.
            // We will throw an error.
            if (node == null) {
                throw new NullPointerException("Node not found");
            }

            NodeOuterClass.GetResponse response = NodeOuterClass.GetResponse.newBuilder()
                    .setSuccess(true)
                    .setNode(convert(node))
                    .build();

            responseObserver.onNext(response);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(Status.INVALID_ARGUMENT.augmentDescription(ex.getLocalizedMessage()).asRuntimeException());
        }
    }

    @Override
    public void getAll(NodeOuterClass.GetAllRequest request, StreamObserver<NodeOuterClass.GetAllResponse> responseObserver) {
        try {
            L4LoadBalancer l4LoadBalancer = getLoadBalancerByID(request.getLoadBalancerId());

            List<NodeOuterClass.Node> nodes = new ArrayList<>();
            l4LoadBalancer.cluster().nodes().forEach(node -> nodes.add(convert(node)));

            NodeOuterClass.GetAllResponse response = NodeOuterClass.GetAllResponse.newBuilder()
                    .setSuccess(true)
                    .addAllNode(nodes)
                    .build();

            responseObserver.onNext(response);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(Status.INVALID_ARGUMENT.augmentDescription(ex.getLocalizedMessage()).asRuntimeException());
        }
    }

    @Override
    public void delete(NodeOuterClass.DeleteRequest request, StreamObserver<NodeOuterClass.DeleteResponse> responseObserver) {
        try {
            L4LoadBalancer l4LoadBalancer = getLoadBalancerByID(request.getLoadBalancerId());
            Node node = null;

            for (Node n : l4LoadBalancer.cluster().nodes()) {
                if (n.id().equalsIgnoreCase(request.getNodeId())) {
                    node = n;
                    break;
                }
            }

            // If node is null then it means we couldn't find the node.
            // We will throw an error.
            if (node == null) {
                throw new NullPointerException("Node not found");
            }

            // Drain connections and remove
            node.drainConnectionAndRemove();

            NodeOuterClass.DeleteResponse response = NodeOuterClass.DeleteResponse.newBuilder()
                    .setSuccess(true)
                    .build();

            responseObserver.onNext(response);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(Status.INVALID_ARGUMENT.augmentDescription(ex.getLocalizedMessage()).asRuntimeException());
        }
    }

    private L4LoadBalancer getLoadBalancerByID(String id) {
        for (Map.Entry<L4LoadBalancer, LoadBalancerProperty> entry : LoadBalancerRegistry.registry.entrySet()) {
            if (entry.getKey().ID.equalsIgnoreCase(id)) {
                return entry.getKey();
            }
        }

        throw new NullPointerException("No LoadBalancer found with ID: " + id);
    }

    private NodeOuterClass.Node convert(Node node) {
        return NodeOuterClass.Node.newBuilder()
                .setId(node.id())
                .setAddress(node.socketAddress().getAddress().getHostAddress())
                .setPort(node.socketAddress().getPort())
                .setActiveConnections(node.activeConnection())
                .setMaxConnections(node.maxConnections())
                .setLoad(node.load())
                .setHealth(node.health().name())
                .setState(node.state().name())
                .build();
    }
}
