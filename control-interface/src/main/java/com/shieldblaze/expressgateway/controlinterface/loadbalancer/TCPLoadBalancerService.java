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
package com.shieldblaze.expressgateway.controlinterface.loadbalancer;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.common.utils.Number;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import com.shieldblaze.expressgateway.core.loadbalancer.LoadBalancerProperty;
import com.shieldblaze.expressgateway.core.loadbalancer.LoadBalancerRegistry;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.grpc.Metadata;
import io.grpc.Status;
import io.grpc.StatusException;
import io.grpc.stub.StreamObserver;

import java.net.InetSocketAddress;
import java.util.Map;
import java.util.concurrent.atomic.AtomicBoolean;

public final class TCPLoadBalancerService extends TCPLoadBalancerServiceGrpc.TCPLoadBalancerServiceImplBase {

    @Override
    public void start(Layer4LoadBalancer.TCPLoadBalancer request, StreamObserver<Layer4LoadBalancer.LoadBalancerResponse> responseObserver) {
        Layer4LoadBalancer.LoadBalancerResponse response;

        try {
            TransportConfiguration transportConfiguration;
            EventLoopConfiguration eventLoopConfiguration;
            BufferConfiguration bufferConfiguration;
            EventStreamConfiguration eventStreamConfiguration;

            TLSConfiguration forServer = null;
            TLSConfiguration forClient = null;

            if (request.getProfileName().equalsIgnoreCase("default") || request.getProfileName().isBlank()) {
                transportConfiguration = TransportConfiguration.DEFAULT;
                eventLoopConfiguration = EventLoopConfiguration.DEFAULT;
                bufferConfiguration = BufferConfiguration.DEFAULT;
                eventStreamConfiguration = EventStreamConfiguration.DEFAULT;
            } else {
                transportConfiguration = TransportConfiguration.loadFrom(request.getProfileName());
                eventLoopConfiguration = EventLoopConfiguration.loadFrom(request.getProfileName());
                bufferConfiguration = BufferConfiguration.loadFrom(request.getProfileName());
                eventStreamConfiguration = EventStreamConfiguration.loadFrom(request.getProfileName());
            }

            if (request.getTlsForServer()) {
                forServer = TLSConfiguration.loadFrom(request.getProfileName(), request.getTlsPassword(), true);
            }

            if (request.getTlsForClient()) {
                forClient = TLSConfiguration.loadFrom(request.getProfileName(), request.getTlsPassword(), false);
            }

            CoreConfiguration configuration = CoreConfigurationBuilder.newBuilder()
                    .withTransportConfiguration(transportConfiguration)
                    .withEventLoopConfiguration(eventLoopConfiguration)
                    .withBufferConfiguration(bufferConfiguration)
                    .build();

            Cluster cluster = new ClusterPool(eventStreamConfiguration.eventStream(), Common.l4(request.getStrategy(), Common.l4(request.getSessionPersistence())));

            L4LoadBalancerBuilder l4LoadBalancerBuilder = L4LoadBalancerBuilder.newBuilder()
                    .withL4FrontListener(new TCPListener())
                    .withBindAddress(new InetSocketAddress(request.getBindAddress(), request.getBindPort()))
                    .withCluster(cluster)
                    .withCoreConfiguration(configuration)
                    .withName(request.getName());

            if (forServer != null) {
                l4LoadBalancerBuilder.withTlsForServer(forServer);
            }

            if (forClient != null) {
                l4LoadBalancerBuilder.withTlsForClient(forClient);
            }

            L4LoadBalancer l4LoadBalancer = l4LoadBalancerBuilder.build();
            L4FrontListenerStartupEvent event = l4LoadBalancer.start();
            LoadBalancerProperty loadBalancerProperty = new LoadBalancerProperty().profileName(request.getProfileName()).startupEvent(event);
            LoadBalancerRegistry.add(l4LoadBalancer, loadBalancerProperty);

            response = Layer4LoadBalancer.LoadBalancerResponse.newBuilder()
                    .setResponseText(l4LoadBalancer.ID)
                    .build();

            responseObserver.onNext(response);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(Status.INVALID_ARGUMENT.augmentDescription(ex.getLocalizedMessage()).asRuntimeException());
        }
    }

    @Override
    public void get(Layer4LoadBalancer.GetLoadBalancerRequest request, StreamObserver<Layer4LoadBalancer.TCPLoadBalancer> responseObserver) {
        try {
            L4LoadBalancer l4LoadBalancer = null;
            LoadBalancerProperty loadBalancerProperty = null;

            for (Map.Entry<L4LoadBalancer, LoadBalancerProperty> entry : LoadBalancerRegistry.registry.entrySet()) {
                if (entry.getKey().ID.equalsIgnoreCase(request.getLoadBalancerId())) {
                    l4LoadBalancer = entry.getKey();
                    loadBalancerProperty = entry.getValue();
                    break;
                }
            }

            if (l4LoadBalancer == null || loadBalancerProperty == null) {
                throw new NullPointerException("Load Balancer was not found");
            }

            // If event has finished and was not successful then
            // remove it from registry and throw an exception.
            if (loadBalancerProperty.startupEvent().finished() && !loadBalancerProperty.startupEvent().success()) {
                LoadBalancerRegistry.remove(l4LoadBalancer);
                throw new IllegalArgumentException("Load Balancer failed to start, Cause: " + loadBalancerProperty.startupEvent().throwable().getLocalizedMessage());
            }

            Layer4LoadBalancer.TCPLoadBalancer response = Layer4LoadBalancer.TCPLoadBalancer.newBuilder()
                    .setBindAddress(l4LoadBalancer.bindAddress().getAddress().getHostAddress())
                    .setBindPort(l4LoadBalancer.bindAddress().getPort())
                    .setTlsForServer(l4LoadBalancer.tlsForServer() != null)
                    .setTlsForClient(l4LoadBalancer.tlsForClient() != null)
                    .setStrategy(l4LoadBalancer.cluster().loadBalance().name())
                    .setSessionPersistence(l4LoadBalancer.cluster().loadBalance().sessionPersistence().name())
                    .setProfileName(loadBalancerProperty.profileName())
                    .build();

            responseObserver.onNext(response);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(Status.INVALID_ARGUMENT.augmentDescription(ex.getLocalizedMessage()).asRuntimeException());
        }
    }

    @Override
    public void stop(Layer4LoadBalancer.StopLoadBalancer request, StreamObserver<Layer4LoadBalancer.LoadBalancerResponse> responseObserver) {
        Layer4LoadBalancer.LoadBalancerResponse response;

        try {
            boolean isFound = false;

            for (Map.Entry<L4LoadBalancer, LoadBalancerProperty> entry : LoadBalancerRegistry.registry.entrySet()) {
                if (entry.getKey().ID.equalsIgnoreCase(request.getId())) {
                    LoadBalancerRegistry.remove(entry.getKey());
                    isFound = true;
                    break;
                }
            }

            if (isFound) {
                response = Layer4LoadBalancer.LoadBalancerResponse.newBuilder()
                        .setResponseText("Success")
                        .build();
            } else {
                throw new NullPointerException("Load Balancer was not found");
            }
        } catch (Exception ex) {
            responseObserver.onError(Status.INVALID_ARGUMENT.augmentDescription(ex.getLocalizedMessage()).asRuntimeException());
            return;
        }

        responseObserver.onNext(response);
        responseObserver.onCompleted();
    }
}
