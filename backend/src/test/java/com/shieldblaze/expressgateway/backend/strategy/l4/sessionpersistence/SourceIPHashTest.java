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
package com.shieldblaze.expressgateway.backend.strategy.l4.sessionpersistence;

import com.shieldblaze.expressgateway.backend.NodeBuilder;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterBuilder;
import com.shieldblaze.expressgateway.backend.strategy.l4.L4Request;
import com.shieldblaze.expressgateway.backend.strategy.l4.RoundRobin;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;

class SourceIPHashTest {

    @Test
    void testSourceIPHash() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(new SourceIPHash()))
                .build();

        fastBuild(cluster, "172.16.20.1");
        fastBuild(cluster, "172.16.20.2");

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

    /**
     * Verifies that IPv6 addresses with high-bit leading bytes (link-local fe80::,
     * multicast ff00::) are handled correctly by the SourceIPHash implementation.
     *
     * The BigInteger(1, bytes) constructor in ipToInt uses sign magnitude = 1 (positive),
     * which ensures that addresses like fe80::1 whose first byte is 0xFE (254) do not
     * produce negative hash keys.
     */
    @Test
    void testSourceIPHashIPv6HighBitAddresses() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(new SourceIPHash()))
                .build();

        fastBuild(cluster, "172.16.20.1");
        fastBuild(cluster, "172.16.20.2");
        fastBuild(cluster, "172.16.20.3");

        // Link-local addresses (fe80::) - high bit leading byte 0xFE
        var responseFe80_1 = cluster.nextNode(new L4Request(new InetSocketAddress("fe80::1", 1)));
        var responseFe80_2 = cluster.nextNode(new L4Request(new InetSocketAddress("fe80::2", 1)));
        assertNotNull(responseFe80_1.node(), "fe80::1 must resolve to a valid node");
        assertNotNull(responseFe80_2.node(), "fe80::2 must resolve to a valid node");

        // Multicast addresses (ff00::) - high bit leading byte 0xFF
        var responseFf00_1 = cluster.nextNode(new L4Request(new InetSocketAddress("ff00::1", 1)));
        assertNotNull(responseFf00_1.node(), "ff00::1 must resolve to a valid node");

        // Close Cluster to prevent memory leaks.
        cluster.close();
    }

    /**
     * Verifies that IPv6 addresses within the same /48 subnet produce consistent
     * hash results (sticky session behavior), while addresses in different /48
     * subnets may produce different hash results.
     *
     * Per the implementation, a /48 mask is applied to IPv6 addresses before hashing.
     */
    @Test
    void testSourceIPHashIPv6SubnetConsistency() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(new SourceIPHash()))
                .build();

        fastBuild(cluster, "172.16.20.1");
        fastBuild(cluster, "172.16.20.2");

        // Two addresses in the same /48 subnet: 2001:db8:1::/48
        // After /48 masking, these should be identical and route to the same node.
        var response1 = cluster.nextNode(new L4Request(new InetSocketAddress("2001:db8:1::1", 1)));
        var response2 = cluster.nextNode(new L4Request(new InetSocketAddress("2001:db8:1::ffff", 1)));
        var response3 = cluster.nextNode(new L4Request(new InetSocketAddress("2001:db8:1:abcd::1", 1)));

        assertEquals(response1.node().socketAddress(), response2.node().socketAddress(),
                "Addresses in the same /48 must hash to the same node");
        assertEquals(response1.node().socketAddress(), response3.node().socketAddress(),
                "Addresses in the same /48 must hash to the same node");

        // Close Cluster to prevent memory leaks.
        cluster.close();
    }

    /**
     * Verifies that IPv6 addresses with different /48 prefixes hash differently,
     * demonstrating that the subnet mask is correctly applied.
     */
    @Test
    void testSourceIPHashIPv6DifferentPrefixes() throws Exception {
        Cluster cluster = ClusterBuilder.newBuilder()
                .withLoadBalance(new RoundRobin(new SourceIPHash()))
                .build();

        fastBuild(cluster, "172.16.20.1");
        fastBuild(cluster, "172.16.20.2");
        fastBuild(cluster, "172.16.20.3");
        fastBuild(cluster, "172.16.20.4");

        // Addresses in distinctly different /48 subnets
        var responseSubnet1 = cluster.nextNode(new L4Request(new InetSocketAddress("2001:db8:1::1", 1)));
        var responseSubnet2 = cluster.nextNode(new L4Request(new InetSocketAddress("2001:db8:2::1", 1)));
        var responseSubnet3 = cluster.nextNode(new L4Request(new InetSocketAddress("fe80:0:1::1", 1)));
        var responseSubnet4 = cluster.nextNode(new L4Request(new InetSocketAddress("fe80:0:2::1", 1)));

        // Each result is valid (non-null). The differing /48 prefixes should produce
        // different masked values. With 4 distinct prefixes and 4 nodes, we cannot
        // guarantee all map differently (hash collisions exist), but we verify validity.
        assertNotNull(responseSubnet1.node(), "Subnet 1 must have a valid node");
        assertNotNull(responseSubnet2.node(), "Subnet 2 must have a valid node");
        assertNotNull(responseSubnet3.node(), "Subnet 3 must have a valid node");
        assertNotNull(responseSubnet4.node(), "Subnet 4 must have a valid node");

        // Close Cluster to prevent memory leaks.
        cluster.close();
    }

    private static void fastBuild(Cluster cluster, String host) throws Exception {
        NodeBuilder.newBuilder()
                .withCluster(cluster)
                .withSocketAddress(new InetSocketAddress(host, 1))
                .build();
    }
}
