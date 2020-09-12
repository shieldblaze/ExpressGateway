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
package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import com.shieldblaze.expressgateway.backend.Backend;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

class SourceIPHashTest {

    @Test
    void getBackendAddress() {
        List<Backend> addressList = new ArrayList<>();

        // Add Backend Server Addresses
        addressList.add(fastBuild("172.16.20.1"));
        addressList.add(fastBuild("172.16.20.2"));

        L4Balance l4Balance = new SourceIPHash(addressList);
        assertEquals(fastBuild("172.16.20.1").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("192.168.1.1", 1)).getInetSocketAddress());
        assertEquals(fastBuild("172.16.20.1").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("192.168.1.23", 1)).getInetSocketAddress());
        assertEquals(fastBuild("172.16.20.1").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("192.168.1.84", 1)).getInetSocketAddress());
        assertEquals(fastBuild("172.16.20.1").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("192.168.1.251", 1)).getInetSocketAddress());

        assertEquals(fastBuild("172.16.20.2").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("10.18.1.10", 1)).getInetSocketAddress());
        assertEquals(fastBuild("172.16.20.2").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("10.18.1.43", 1)).getInetSocketAddress());
        assertEquals(fastBuild("172.16.20.2").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("10.18.1.72", 1)).getInetSocketAddress());
        assertEquals(fastBuild("172.16.20.2").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("10.18.1.213", 1)).getInetSocketAddress());

        assertEquals(fastBuild("172.16.20.1").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("127.0.0.10", 1)).getInetSocketAddress());
        assertEquals(fastBuild("172.16.20.1").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("127.0.0.172", 1)).getInetSocketAddress());
        assertEquals(fastBuild("172.16.20.1").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("127.0.0.230", 1)).getInetSocketAddress());
        assertEquals(fastBuild("172.16.20.1").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("127.0.0.253", 1)).getInetSocketAddress());

        assertEquals(fastBuild("172.16.20.2").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("172.20.1.10", 1)).getInetSocketAddress());
        assertEquals(fastBuild("172.16.20.2").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("172.20.1.172", 1)).getInetSocketAddress());
        assertEquals(fastBuild("172.16.20.2").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("172.20.1.230", 1)).getInetSocketAddress());
        assertEquals(fastBuild("172.16.20.2").getInetSocketAddress(),
                l4Balance.getBackend(new InetSocketAddress("172.20.1.253", 1)).getInetSocketAddress());
    }

    private Backend fastBuild(String host) {
        return new Backend(new InetSocketAddress(host, 1), 1, 0);
    }
}
