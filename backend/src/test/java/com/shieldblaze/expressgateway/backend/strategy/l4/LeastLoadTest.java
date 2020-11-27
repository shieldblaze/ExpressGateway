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
package com.shieldblaze.expressgateway.backend.strategy.l4;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.*;

class LeastLoadTest {

    @Test
    void testLeastLoad() throws LoadBalanceException {
        EventStream eventStream = new EventStream();

        ClusterPool cluster = new ClusterPool(eventStream, new LeastLoad(new NOOPSessionPersistence()));
        fastBuild(cluster, "10.10.1.1", 100_000);
        fastBuild(cluster, "10.10.1.2", 200_000);
        fastBuild(cluster, "10.10.1.3", 300_000);
        fastBuild(cluster, "10.10.1.4", 400_000);

        int first = 0;
        int second = 0;
        int third = 0;
        int forth = 0;

        L4Request l4Request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        for (int i = 0; i < 1_000_000; i++) {
            Node node = cluster.nextNode(l4Request).node();
            node.incActiveConnection0();
            switch (node.socketAddress().getHostString()) {
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

        assertEquals(100_000, first);
        assertEquals(200_000, second);
        assertEquals(300_000, third);
        assertEquals(400_000, forth);
    }

    private static Node fastBuild(Cluster cluster, String host, int maxConnections) {
        return new Node(cluster, new InetSocketAddress(host, 1), maxConnections, null);
    }
}
