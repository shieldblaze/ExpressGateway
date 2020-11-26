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
package com.shieldblaze.expressgateway.backend.strategy.l7.http;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import io.netty.handler.codec.http.EmptyHttpHeaders;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;

class HTTPWeightedHTTPRoundRobinTest {

    @Test
    void testWeightedRoundRobin() throws LoadBalanceException {

        Cluster cluster = new ClusterPool(new EventStream(), new HTTPWeightedRoundRobin(new NOOPSessionPersistence()));
        cluster.addNode(fastBuild(cluster, "10.10.1.1", 10));
        cluster.addNode(fastBuild(cluster, "10.10.1.2", 20));
        cluster.addNode(fastBuild(cluster, "10.10.1.3", 30));
        cluster.addNode(fastBuild(cluster, "10.10.1.4", 40));

        int first = 0;
        int second = 0;
        int third = 0;
        int forth = 0;

        HTTPBalanceRequest httpBalanceRequest = new HTTPBalanceRequest(new InetSocketAddress("192.168.1.1", 1), EmptyHttpHeaders.INSTANCE);

        for (int i = 0; i < 1000000; i++) {
            switch (cluster.nextNode(httpBalanceRequest).node().socketAddress().getHostString()) {
                case "10.10.1.1": {
                    first++;
                    break;
                }
                case "10.10.1.2": {
                    second++;
                    break;
                }
                case "10.10.1.3": {
                    third++;
                    break;
                }
                case "10.10.1.4": {
                    forth++;
                    break;
                }
                default:
                    break;
            }
        }

        assertEquals(100000, first);
        assertEquals(200000, second);
        assertEquals(300000, third);
        assertEquals(400000, forth);
    }

    private Node fastBuild(Cluster cluster, String host, int weight) {
        return new Node(cluster, new InetSocketAddress(host, 1), weight, 1, null);
    }
}
