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
package com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Request;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;

class SourceIPHashTest {

    @Test
    void testSourceIPHash() throws LoadBalanceException {

        Cluster cluster = new ClusterPool(new EventStream(), new RoundRobin(new SourceIPHash()));
        cluster.addNode(fastBuild(cluster, "172.16.20.1"));
        cluster.addNode(fastBuild(cluster, "172.16.20.2"));

        assertEquals(new InetSocketAddress("172.16.20.1", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("192.168.1.1", 1))).node().socketAddress());
        assertEquals(new InetSocketAddress("172.16.20.1", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("192.168.1.23", 1))).node().socketAddress());
        assertEquals(new InetSocketAddress("172.16.20.1", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("192.168.1.84", 1))).node().socketAddress());
        assertEquals(new InetSocketAddress("172.16.20.1", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("192.168.1.251", 1))).node().socketAddress());

        assertEquals(new InetSocketAddress("172.16.20.2", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("10.18.1.10", 1))).node().socketAddress());
        assertEquals(new InetSocketAddress("172.16.20.2", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("10.18.1.43", 1))).node().socketAddress());
        assertEquals(new InetSocketAddress("172.16.20.2", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("10.18.1.72", 1))).node().socketAddress());
        assertEquals(new InetSocketAddress("172.16.20.2", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("10.18.1.213", 1))).node().socketAddress());

        assertEquals(new InetSocketAddress("172.16.20.1", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("127.0.0.10", 1))).node().socketAddress());
        assertEquals(new InetSocketAddress("172.16.20.1", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("127.0.0.172", 1))).node().socketAddress());
        assertEquals(new InetSocketAddress("172.16.20.1", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("127.0.0.230", 1))).node().socketAddress());
        assertEquals(new InetSocketAddress("172.16.20.1", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("127.0.0.253", 1))).node().socketAddress());

        assertEquals(new InetSocketAddress("172.16.20.2", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("172.20.1.10", 1))).node().socketAddress());
        assertEquals(new InetSocketAddress("172.16.20.2", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("172.20.1.172", 1))).node().socketAddress());
        assertEquals(new InetSocketAddress("172.16.20.2", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("172.20.1.230", 1))).node().socketAddress());
        assertEquals(new InetSocketAddress("172.16.20.2", 1),
                cluster.nextNode(new L4Request(new InetSocketAddress("172.20.1.253", 1))).node().socketAddress());
    }

    private Node fastBuild(Cluster cluster, String host) {
        return new Node(cluster, new InetSocketAddress(host, 1));
    }
}
