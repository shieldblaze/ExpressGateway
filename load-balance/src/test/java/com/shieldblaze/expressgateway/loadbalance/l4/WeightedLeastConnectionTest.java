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
package com.shieldblaze.expressgateway.loadbalance.l4;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.loadbalance.exceptions.LoadBalanceException;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;

class WeightedLeastConnectionTest {

    private static Backend fastBuild(String host, int weight) {
        return new Backend(new InetSocketAddress(host, 1), weight, 1);
    }

    @Test
    void testWeightedLeastConnection() throws LoadBalanceException {
        Cluster cluster = ClusterPool.of(
                "localhost.domain",
                fastBuild("10.10.1.1", 10),
                fastBuild("10.10.1.2", 20),
                fastBuild("10.10.1.3", 30),
                fastBuild("10.10.1.4", 40)
        );

        int first = 0;
        int second = 0;
        int third = 0;
        int forth = 0;

        L4Balance l4Balance = new WeightedLeastConnection(cluster);
        L4Request l4Request = new L4Request(new InetSocketAddress("192.168.1.1", 1));

        for (int i = 0; i < 1000000; i++) {
            Backend backend = l4Balance.getResponse(l4Request).getBackend();
            backend.incConnections();
            switch (backend.getSocketAddress().getHostString()) {
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
}
