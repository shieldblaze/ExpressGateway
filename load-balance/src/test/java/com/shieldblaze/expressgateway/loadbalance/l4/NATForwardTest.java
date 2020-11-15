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
import com.shieldblaze.expressgateway.backend.cluster.ClusterPool;
import com.shieldblaze.expressgateway.backend.cluster.SingleBackendCluster;
import com.shieldblaze.expressgateway.backend.exceptions.LoadBalanceException;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class NATForwardTest {

    @Test
    void testNATForward() throws LoadBalanceException {

        L4Balance l4Balance = new NATForward(SingleBackendCluster.of("localhost.domain", fastBuild("192.168.1.1")));

        assertEquals(fastBuild("192.168.1.1").socketAddress(),
                l4Balance.response(new L4Request(new InetSocketAddress("10.10.10.1", 1))).backend().socketAddress());
        assertEquals(fastBuild("192.168.1.1").socketAddress(),
                l4Balance.response(new L4Request(new InetSocketAddress("10.10.10.2", 2))).backend().socketAddress());
        assertEquals(fastBuild("192.168.1.1").socketAddress(),
                l4Balance.response(new L4Request(new InetSocketAddress("10.10.10.3", 3))).backend().socketAddress());
        assertEquals(fastBuild("192.168.1.1").socketAddress(),
                l4Balance.response(new L4Request(new InetSocketAddress("10.10.10.4", 4))).backend().socketAddress());
    }

    @Test
    void throwException() {
        assertThrows(IllegalArgumentException.class, () -> {
            new NATForward(ClusterPool.of("localhost", fastBuild("192.168.1.1"), fastBuild("192.168.1.2")));
        });
    }

    private Backend fastBuild(String host) {
        return new Backend(new InetSocketAddress(host, 1));
    }
}
