/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence.NOOPSessionPersistence;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;

class LeastConnectionTest {

    @Test
    void testLeastConnection() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new LeastConnection(NOOPSessionPersistence.INSTANCE))
                .build();

        fastBuild(cluster, "10.10.1.1");
        fastBuild(cluster, "10.10.1.2");
        fastBuild(cluster, "10.10.1.3");
        fastBuild(cluster, "10.10.1.4");

        int first = 0;
        int second = 0;
        int third = 0;
        int forth = 0;

        L4Request l4Request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        for (int i = 0; i < 1_000_000; i++) {
            Node node = cluster.nextNode(l4Request).node();
            node.incActiveConnection0();
            switch (node.socketAddress().getHostString()) {
                case "10.10.1.1" -> {
                    first++;
                }
                case "10.10.1.2" -> {
                    second++;
                }
                case "10.10.1.3" -> {
                    third++;
                }
                case "10.10.1.4" -> {
                    forth++;
                }
                default -> {
                }
            }
        }

        assertEquals(250_000, first);
        assertEquals(250_000, second);
        assertEquals(250_000, third);
        assertEquals(250_000, forth);
    }

    private static void fastBuild(Cluster cluster, String host) throws Exception {
        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(host, 1))
                .build();
    }
}
