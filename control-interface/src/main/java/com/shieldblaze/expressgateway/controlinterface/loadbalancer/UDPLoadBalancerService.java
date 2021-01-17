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
import com.shieldblaze.expressgateway.configuration.CoreConfiguration;
import com.shieldblaze.expressgateway.configuration.CoreConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.buffer.BufferConfiguration;
import com.shieldblaze.expressgateway.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.configuration.eventstream.EventStreamConfiguration;
import com.shieldblaze.expressgateway.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.core.events.L4FrontListenerStartupEvent;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancer;
import com.shieldblaze.expressgateway.core.loadbalancer.L4LoadBalancerBuilder;
import com.shieldblaze.expressgateway.core.loadbalancer.LoadBalancerRegistry;
import com.shieldblaze.expressgateway.protocol.udp.UDPListener;
import io.grpc.stub.StreamObserver;

import java.net.InetSocketAddress;

public final class UDPLoadBalancerService extends UDPLoadBalancerServiceGrpc.UDPLoadBalancerServiceImplBase {

    @Override
    public void udp(Layer4LoadBalancer.UDPLoadBalancer request, StreamObserver<Layer4LoadBalancer.LoadBalancerResponse> responseObserver) {
        Layer4LoadBalancer.LoadBalancerResponse response;

        try {

            TransportConfiguration transportConfiguration = TransportConfiguration.loadFrom(request.getProfileName());
            EventLoopConfiguration eventLoopConfiguration = EventLoopConfiguration.loadFrom(request.getProfileName());
            BufferConfiguration bufferConfiguration = BufferConfiguration.loadFrom(request.getProfileName());
            EventStreamConfiguration eventStreamConfiguration = EventStreamConfiguration.loadFrom(request.getProfileName());

            CoreConfiguration configuration = CoreConfigurationBuilder.newBuilder()
                    .withTransportConfiguration(transportConfiguration)
                    .withEventLoopConfiguration(eventLoopConfiguration)
                    .withBufferConfiguration(bufferConfiguration)
                    .build();

            Cluster cluster = new ClusterPool(eventStreamConfiguration.eventStream(), Utils.l4(request.getStrategy(), Utils.l4(request.getSessionPersistence())));

            L4LoadBalancerBuilder l4LoadBalancerBuilder = L4LoadBalancerBuilder.newBuilder()
                    .withL4FrontListener(new UDPListener())
                    .withBindAddress(new InetSocketAddress(request.getBindAddress(), request.getBindPort()))
                    .withCluster(cluster)
                    .withCoreConfiguration(configuration);

            L4LoadBalancer l4LoadBalancer = l4LoadBalancerBuilder.build();
            L4FrontListenerStartupEvent event = l4LoadBalancer.start();
            LoadBalancerRegistry.add(l4LoadBalancer, event);

            response = Layer4LoadBalancer.LoadBalancerResponse.newBuilder()
                    .setSuccess(true)
                    .setResponseText(l4LoadBalancer.ID)
                    .build();
        } catch (Exception ex) {
            response = Layer4LoadBalancer.LoadBalancerResponse.newBuilder()
                    .setSuccess(false)
                    .setResponseText(ex.getLocalizedMessage())
                    .build();
        }

        responseObserver.onNext(response);
        responseObserver.onCompleted();
    }

    @Override
    public void stopUDP(Layer4LoadBalancer.StopLoadBalancer request, StreamObserver<Layer4LoadBalancer.LoadBalancerResponse> responseObserver) {
        responseObserver.onNext(Utils.stopLoadBalancer(request));
        responseObserver.onCompleted();
    }
}
