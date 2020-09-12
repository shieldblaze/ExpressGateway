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

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;

class RoundRobinTest {

    @Test
    void getBackendAddress() {
        List<Backend> addressList = new ArrayList<>();

        // Add Backend Server Addresses
        for (int i = 1; i <= 100; i++) {
            addressList.add(fastBuild("192.168.1." + i));
        }

        L4Balance l4Balance = new RoundRobin(addressList);

        for (int i = 1; i <= 100; i++) {
            assertEquals(fastBuild("192.168.1." + i).getInetSocketAddress(), l4Balance.getBackend(null).getInetSocketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertEquals(fastBuild("192.168.1." + i).getInetSocketAddress(), l4Balance.getBackend(null).getInetSocketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertNotEquals(fastBuild("10.10.1." + i).getInetSocketAddress(), l4Balance.getBackend(null).getInetSocketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertNotEquals(fastBuild("172.16.20." + i).getInetSocketAddress(), l4Balance.getBackend(null).getInetSocketAddress());
        }
    }

    private Backend fastBuild(String host) {
        return new Backend(new InetSocketAddress(host, 1), 1, 0);
    }
}
