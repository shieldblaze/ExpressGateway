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
package com.shieldblaze.expressgateway.loadbalance.l7.http.sessionpersistence;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.cluster.Cluster;
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.loadbalance.exceptions.LoadBalanceException;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.loadbalance.l7.http.HTTPBalanceResponse;
import com.shieldblaze.expressgateway.loadbalance.l7.http.RoundRobin;
import io.netty.handler.codec.http.EmptyHttpHeaders;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.*;

class StickySessionTest {

    @Test
    void testStickySession() throws LoadBalanceException {
        Cluster cluster = ClusterPool.of(
                new Backend(new InetSocketAddress("172.16.1.1", 9110)),
                new Backend(new InetSocketAddress("172.16.1.2", 9110)),
                new Backend(new InetSocketAddress("172.16.1.3", 9110)),
                new Backend(new InetSocketAddress("172.16.1.4", 9110))
        );

        for (int i = 0; i < 100; i++) {
            InetSocketAddress socketAddress = new InetSocketAddress("192.168.1." + i,1);
            HTTPBalanceRequest httpBalanceRequest = new HTTPBalanceRequest(socketAddress, EmptyHttpHeaders.INSTANCE);

            RoundRobin roundRobin = new RoundRobin(new StickySession(cluster.getOnlineBackends()), cluster);
            HTTPBalanceResponse httpBalanceResponse = roundRobin.getResponse(httpBalanceRequest);
            assertEquals(cluster.get(0), httpBalanceResponse.getBackend());

            httpBalanceRequest = new HTTPBalanceRequest(socketAddress, httpBalanceResponse.getHTTPHeaders());
            httpBalanceResponse = roundRobin.getResponse(httpBalanceRequest);
            assertEquals(cluster.get(1),  httpBalanceResponse.getBackend());

            httpBalanceRequest = new HTTPBalanceRequest(socketAddress, httpBalanceResponse.getHTTPHeaders());
            httpBalanceResponse = roundRobin.getResponse(httpBalanceRequest);
            assertEquals(cluster.get(2),  httpBalanceResponse.getBackend());

            httpBalanceRequest = new HTTPBalanceRequest(socketAddress, httpBalanceResponse.getHTTPHeaders());
            httpBalanceResponse = roundRobin.getResponse(httpBalanceRequest);
            assertEquals(cluster.get(3),  httpBalanceResponse.getBackend());
        }
    }
}
