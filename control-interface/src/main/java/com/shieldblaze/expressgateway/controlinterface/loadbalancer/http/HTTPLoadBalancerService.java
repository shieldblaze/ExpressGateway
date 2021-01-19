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
package com.shieldblaze.expressgateway.controlinterface.loadbalancer.http;

import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.http.HTTPConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.controlinterface.loadbalancer.Common;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.LoadBalancerProperty;
import com.shieldblaze.expressgateway.core.loadbalancer.LoadBalancerRegistry;
import com.shieldblaze.expressgateway.protocol.http.DefaultHTTPServerInitializer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;
import io.grpc.Status;
import io.grpc.stub.StreamObserver;

import java.net.InetSocketAddress;
import java.util.Map;

public class HTTPLoadBalancerService extends HTTPLoadBalancerServiceGrpc.HTTPLoadBalancerServiceImplBase {

    @Override
    public void start(HTTPLoadBalancerOuterClass.HTTPLoadBalancer request, StreamObserver<HTTPLoadBalancerOuterClass.LoadBalancerResponse> responseObserver) {
        HTTPLoadBalancerOuterClass.LoadBalancerResponse response;

        try {
            TransportConfiguration transportConfiguration;
            EventLoopConfiguration eventLoopConfiguration;
            BufferConfiguration bufferConfiguration;
            EventStreamConfiguration eventStreamConfiguration;
            HTTPConfiguration httpConfiguration;

            if (request.getProfileName().equalsIgnoreCase("default")) {
                transportConfiguration = TransportConfiguration.DEFAULT;
                eventLoopConfiguration = EventLoopConfiguration.DEFAULT;
                bufferConfiguration = BufferConfiguration.DEFAULT;
                eventStreamConfiguration = EventStreamConfiguration.DEFAULT;
                httpConfiguration = HTTPConfiguration.DEFAULT;
            } else {
                transportConfiguration = TransportConfiguration.loadFrom(request.getProfileName());
                eventLoopConfiguration = EventLoopConfiguration.loadFrom(request.getProfileName());
                bufferConfiguration = BufferConfiguration.loadFrom(request.getProfileName());
                eventStreamConfiguration = EventStreamConfiguration.loadFrom(request.getProfileName());
                httpConfiguration = HTTPConfiguration.loadFrom(request.getProfileName());
            }

            CoreConfiguration configuration = CoreConfigurationBuilder.newBuilder()
                    .withTransportConfiguration(transportConfiguration)
                    .withEventLoopConfiguration(eventLoopConfiguration)
                    .withBufferConfiguration(bufferConfiguration)
                    .build();

            TLSConfiguration forServer = null;
            TLSConfiguration forClient = null;
            if (request.getTlsForServer()) {
                forServer = TLSConfiguration.loadFrom(request.getProfileName(), request.getTlsPassword(), true);
            }

            if (request.getTlsForClient()) {
                forClient = TLSConfiguration.loadFrom(request.getProfileName(), request.getTlsPassword(), false);
            }

            Cluster cluster = new ClusterPool(eventStreamConfiguration.eventStream(), Common.l7(request.getStrategy(), Common.l7(request.getSessionPersistence())));

            HTTPLoadBalancerBuilder httpLoadBalancerBuilder = HTTPLoadBalancerBuilder.newBuilder()
                    .withL4FrontListener(new TCPListener())
                    .withHTTPInitializer(new DefaultHTTPServerInitializer())
                    .withBindAddress(new InetSocketAddress(request.getBindAddress(), request.getBindPort()))
                    .withCluster(cluster)
                    .withCoreConfiguration(configuration)
                    .withHTTPConfiguration(httpConfiguration)
                    .withName(request.getName());

            if (forServer != null) {
                httpLoadBalancerBuilder.withTLSForServer(forServer);
            }

            if (forClient != null) {
                httpLoadBalancerBuilder.withTLSForClient(forClient);
            }

            HTTPLoadBalancer httpLoadBalancer = httpLoadBalancerBuilder.build();
            L4FrontListenerStartupEvent event = httpLoadBalancer.start();
            LoadBalancerProperty loadBalancerProperty = new LoadBalancerProperty().profileName(request.getProfileName()).startupEvent(event);
            LoadBalancerRegistry.add(httpLoadBalancer, loadBalancerProperty);

            response = HTTPLoadBalancerOuterClass.LoadBalancerResponse.newBuilder()
                    .setResponseText(httpLoadBalancer.ID)
                    .build();
        } catch (Exception ex) {
            responseObserver.onError(Status.INVALID_ARGUMENT.augmentDescription(ex.getLocalizedMessage()).asRuntimeException());
            return;
        }

        responseObserver.onNext(response);
        responseObserver.onCompleted();
    }

    @Override
    public void get(HTTPLoadBalancerOuterClass.GetLoadBalancerRequest request, StreamObserver<HTTPLoadBalancerOuterClass.HTTPLoadBalancer> responseObserver) {
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

            HTTPLoadBalancerOuterClass.HTTPLoadBalancer response = HTTPLoadBalancerOuterClass.HTTPLoadBalancer.newBuilder()
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
    public void stop(HTTPLoadBalancerOuterClass.StopLoadBalancer request, StreamObserver<HTTPLoadBalancerOuterClass.LoadBalancerResponse> responseObserver) {
        HTTPLoadBalancerOuterClass.LoadBalancerResponse response;

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
                response = HTTPLoadBalancerOuterClass.LoadBalancerResponse.newBuilder()
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
