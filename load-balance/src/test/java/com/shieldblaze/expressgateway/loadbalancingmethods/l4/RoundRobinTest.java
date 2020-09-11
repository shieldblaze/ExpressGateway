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
