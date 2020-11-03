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
package com.shieldblaze.expressgateway.loadbalance.l7.http;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import io.netty.handler.codec.http.EmptyHttpHeaders;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertTrue;

class WeightedRandomTest {

    @Test
    void testWeightedRandom() throws LoadBalanceException {

        Cluster cluster = ClusterPool.of(
                "localhost.domain",
                fastBuild("10.10.1.1", 30),
                fastBuild("10.10.1.2", 20),
                fastBuild("10.10.1.3", 40),
                fastBuild("10.10.1.4", 10)
        );

        int first = 0;
        int second = 0;
        int third = 0;
        int forth = 0;

        HTTPBalance httpBalance = new WeightedRandom(cluster);
        HTTPBalanceRequest httpBalanceRequest = new HTTPBalanceRequest(new InetSocketAddress("192.168.1.1", 1), EmptyHttpHeaders.INSTANCE);

        for (int i = 0; i < 1000; i++) {
            switch (httpBalance.getResponse(httpBalanceRequest).getBackend().getSocketAddress().getHostString()) {
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

        assertTrue(first > 200);
        assertTrue(second > 100);
        assertTrue(third > 300);
        assertTrue(forth > 75);
    }

    private Backend fastBuild(String host, int weight) {
        return new Backend(new InetSocketAddress(host, 1), weight, 1);
    }
}
