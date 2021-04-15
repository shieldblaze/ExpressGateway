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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRandom;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancer;
import com.shieldblaze.expressgateway.protocol.http.loadbalancer.HTTPLoadBalancerBuilder;
import com.shieldblaze.expressgateway.protocol.tcp.TCPListener;

import java.net.InetSocketAddress;

public class Main {

    static {
        System.setProperty("log4j.configurationFile", "log4j2.xml");
    }

    public static void main(String[] args) {
        HTTPLoadBalancer httpLoadBalancer = HTTPLoadBalancerBuilder.newBuilder()
                .withBindAddress(new InetSocketAddress("0.0.0.0", 9110))
                .withL4FrontListener(new TCPListener())
                .withHTTPInitializer(new DefaultHTTPServerInitializer())
                .build();

        httpLoadBalancer.start();

        ClusterPool clusterPool = new ClusterPool(new HTTPRandom(NOOPSessionPersistence.INSTANCE));
        httpLoadBalancer.mapCluster("192.168.133.197:9110", clusterPool);

        new Node(clusterPool, new InetSocketAddress("172.27.5.153", 80));
    }
}
