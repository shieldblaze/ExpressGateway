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
package com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceResponse;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPRoundRobin;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import io.netty.handler.codec.http.EmptyHttpHeaders;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;

class StickySessionTest {

    @Test
    void testStickySession() throws LoadBalanceException {
        Cluster cluster = new ClusterPool(new EventStream(), new HTTPRoundRobin(new StickySession()));
        fastBuild(cluster, "172.16.20.1");
        fastBuild(cluster, "172.16.20.2");
        fastBuild(cluster, "172.16.20.3");
        fastBuild(cluster, "172.16.20.4");

        for (int i = 0; i < 100; i++) {
            InetSocketAddress socketAddress = new InetSocketAddress("192.168.1." + i, 1);
            HTTPBalanceRequest httpBalanceRequest = new HTTPBalanceRequest(socketAddress, EmptyHttpHeaders.INSTANCE);

            HTTPBalanceResponse httpBalanceResponse = (HTTPBalanceResponse) cluster.nextNode(httpBalanceRequest);
            assertEquals(cluster.nodes().get(0), httpBalanceResponse.node());

            httpBalanceRequest = new HTTPBalanceRequest(socketAddress, httpBalanceResponse.getHTTPHeaders());
            httpBalanceResponse = (HTTPBalanceResponse) cluster.nextNode(httpBalanceRequest);
            assertEquals(cluster.nodes().get(1), httpBalanceResponse.node());

            httpBalanceRequest = new HTTPBalanceRequest(socketAddress, httpBalanceResponse.getHTTPHeaders());
            httpBalanceResponse = (HTTPBalanceResponse) cluster.nextNode(httpBalanceRequest);
            assertEquals(cluster.nodes().get(2), httpBalanceResponse.node());

            httpBalanceRequest = new HTTPBalanceRequest(socketAddress, httpBalanceResponse.getHTTPHeaders());
            httpBalanceResponse = (HTTPBalanceResponse) cluster.nextNode(httpBalanceRequest);
            assertEquals(cluster.nodes().get(3), httpBalanceResponse.node());
        }
    }

    private Node fastBuild(Cluster cluster, String host) {
        return new Node(cluster, new InetSocketAddress(host, 1));
    }
}
