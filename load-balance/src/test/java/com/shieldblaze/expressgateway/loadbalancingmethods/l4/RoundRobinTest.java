package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import com.shieldblaze.expressgateway.backend.Backend;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

class RoundRobinTest {

    @Test
    void getBackendAddress() {
        List<Backend> addressList = new ArrayList<>();

        // Add Backend Server Addresses
        for (int i = 1; i <= 100; i++) {
            addressList.add(new Backend(new InetSocketAddress("192.168.1." + i, i), 1, 0));
        }

        L4Balance l4Balance = new RoundRobin(addressList);

        for (int i = 1; i <= 100; i++) {
            assertEquals(new Backend(new InetSocketAddress("192.168.1." + i, i), 1, 0).getInetSocketAddress(),
                    l4Balance.getBackend(null).getInetSocketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertEquals(new Backend(new InetSocketAddress("192.168.1." + i, i), 1, 0).getInetSocketAddress(),
                    l4Balance.getBackend(null).getInetSocketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertNotEquals(new Backend(new InetSocketAddress("10.10.1." + i, i), 1, 0).getInetSocketAddress(),
                    l4Balance.getBackend(null).getInetSocketAddress());
        }

        for (int i = 1; i <= 100; i++) {
            assertNotEquals(new Backend(new InetSocketAddress("172.16.20." + i, i), 1, 0).getInetSocketAddress(),
                    l4Balance.getBackend(null).getInetSocketAddress());
        }
    }
}
