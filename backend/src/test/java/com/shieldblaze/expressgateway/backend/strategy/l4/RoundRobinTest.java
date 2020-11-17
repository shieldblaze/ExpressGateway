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
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;

class RoundRobinTest {

    @Test
    void testRoundRobin() throws LoadBalanceException {
        ClusterPool cluster = new ClusterPool();

        // Add Backend Server Addresses
        for (int i = 1; i <= 100; i++) {
            cluster.addBackends(fastBuild("192.168.1." + i));
        }

        L4Balance l4Balance = new RoundRobin(cluster);
        L4Request l4Request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        for (int i = 1; i <= 100; i++) {
            assertEquals(fastBuild("192.168.1." + i).socketAddress(), l4Balance.response(l4Request).backend().socketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertEquals(fastBuild("192.168.1." + i).socketAddress(), l4Balance.response(l4Request).backend().socketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertNotEquals(fastBuild("10.10.1." + i).socketAddress(), l4Balance.response(l4Request).backend().socketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertNotEquals(fastBuild("172.16.20." + i).socketAddress(), l4Balance.response(l4Request).backend().socketAddress());
        }
    }

    private Node fastBuild(String host) {
        return new Node(new InetSocketAddress(host, 1));
    }
}
