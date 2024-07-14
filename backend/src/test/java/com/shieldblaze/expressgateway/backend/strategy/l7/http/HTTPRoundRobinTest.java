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
package com.shieldblaze.expressgateway.backend.strategy.l7.http;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence.NOOPSessionPersistence;
import io.netty.handler.codec.http.EmptyHttpHeaders;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;

class HTTPRoundRobinTest {

    @Test
    void testRoundRobin() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder().withLoadBalance(new HTTPRoundRobin(NOOPSessionPersistence.INSTANCE)).build();

        // Add Node Server Addresses
        for (int i = 1; i <= 100; i++) {
            fastBuild(cluster, "192.168.1." + i);
        }

        HTTPBalanceRequest httpBalanceRequest = new HTTPBalanceRequest(new InetSocketAddress("192.168.1.1", 1), EmptyHttpHeaders.INSTANCE);

        for (int i = 1; i <= 100; i++) {
            assertEquals(new InetSocketAddress("192.168.1." + i, 1), cluster.nextNode(httpBalanceRequest).node().socketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertEquals(new InetSocketAddress("192.168.1." + i, 1), cluster.nextNode(httpBalanceRequest).node().socketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertNotEquals(new InetSocketAddress("10.10.1." + i, 1), cluster.nextNode(httpBalanceRequest).node().socketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertNotEquals(new InetSocketAddress("172.16.20." + i, 1), cluster.nextNode(httpBalanceRequest).node().socketAddress());
        }
    }

    private static void fastBuild(Cluster cluster, String host) throws Exception {
        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(host, 1))
                .build();
    }
}
