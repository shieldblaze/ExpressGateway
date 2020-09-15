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
package com.shieldblaze.expressgateway.core;

import com.shieldblaze.expressgateway.core.configuration.Configuration;
import com.shieldblaze.expressgateway.core.configuration.ConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.core.configuration.eventloop.EventLoopConfiguration;
import com.shieldblaze.expressgateway.core.configuration.eventloop.EventLoopConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.transport.ReceiveBufferAllocationType;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfiguration;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportConfigurationBuilder;
import com.shieldblaze.expressgateway.core.configuration.transport.TransportType;
import com.shieldblaze.expressgateway.core.loadbalance.backend.Backend;
import com.shieldblaze.expressgateway.core.loadbalance.l4.RoundRobin;
import com.shieldblaze.expressgateway.core.server.udp.UDPListener;
import com.shieldblaze.expressgateway.core.loadbalance.backend.Cluster;

import java.net.InetSocketAddress;

public final class Main {

    static {
        System.setProperty("log4j.configurationFile", "log4j2.xml");
    }

    public static void main(String[] args) {
        TransportConfiguration transportConfiguration = TransportConfigurationBuilder.newBuilder()
                .withTransportType(TransportType.NIO)
                .withTCPFastOpenMaximumPendingRequests(2147483647)
                .withBackendConnectTimeout(1000 * 5)
                .withListenerSocketTimeout(1000 * 5)
                .withReceiveBufferAllocationType(ReceiveBufferAllocationType.FIXED)
                .withReceiveBufferSizes(new int[]{100})
                .withSocketReceiveBufferSize(2147483647)
                .withSocketSendBufferSize(2147483647)
                .withTCPConnectionBacklog(2147483647)
                .withDataBacklog(2147483647)
                .build();

        EventLoopConfiguration eventLoopConfiguration = EventLoopConfigurationBuilder.newBuilder()
                .withParentWorkers(2)
                .withChildWorkers(4)
                .build();

        Configuration configuration = ConfigurationBuilder.newBuilder()
                .withTransportConfiguration(transportConfiguration)
                .withEventLoopConfiguration(eventLoopConfiguration)
                .withPooledByteBufAllocatorConfiguration(PooledByteBufAllocatorConfiguration.DEFAULT)
                .build();

        Cluster cluster = new Cluster();
        cluster.setClusterName("MyCluster");
        cluster.addBackend(new Backend(new InetSocketAddress("127.0.0.1", 9111)));

        L4LoadBalancer l4LoadBalancer = L4LoadBalancerBuilder.newBuilder()
                .withConfiguration(configuration)
                .withL4Balance(new RoundRobin())
                .withCluster(cluster)
                .withFrontListener(new UDPListener(new InetSocketAddress("0.0.0.0", 9110)))
                .build();

        l4LoadBalancer.start();
    }
}
