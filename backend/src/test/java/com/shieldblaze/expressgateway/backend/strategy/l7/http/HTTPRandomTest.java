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

import static org.junit.jupiter.api.Assertions.assertTrue;

class HTTPRandomTest {

    @Test
    void testRandom() throws LoadBalanceException {
        Cluster cluster = new ClusterPool(new EventStream(), new HTTPRandom(new NOOPSessionPersistence()));
        cluster.addNode(fastBuild(cluster, "172.16.20.1"));
        cluster.addNode(fastBuild(cluster, "172.16.20.2"));
        cluster.addNode(fastBuild(cluster, "172.16.20.3"));
        cluster.addNode(fastBuild(cluster, "172.16.20.4"));
        cluster.addNode(fastBuild(cluster, "172.16.20.5"));

        HTTPBalanceRequest httpBalanceRequest = new HTTPBalanceRequest(new InetSocketAddress("192.168.1.1", 1), EmptyHttpHeaders.INSTANCE);

        int first = 0;
        int second = 0;
        int third = 0;
        int forth = 0;
        int fifth = 0;

        for (int i = 0; i < 1000; i++) {
            switch (cluster.nextNode(httpBalanceRequest).node().socketAddress().getHostString()) {
                case "172.16.20.1": {
                    first++;
                    break;
                }
                case "172.16.20.2": {
                    second++;
                    break;
                }
                case "172.16.20.3": {
                    third++;
                    break;
                }
                case "172.16.20.4": {
                    forth++;
                    break;
                }
                case "172.16.20.5": {
                    fifth++;
                    break;
                }
                default:
                    break;
            }
        }

        assertTrue(first > 10);
        assertTrue(second > 10);
        assertTrue(third > 10);
        assertTrue(forth > 10);
        assertTrue(fifth > 10);
    }

    private Node fastBuild(Cluster cluster, String host) {
        return new Node(cluster, new InetSocketAddress(host, 1));
    }
}