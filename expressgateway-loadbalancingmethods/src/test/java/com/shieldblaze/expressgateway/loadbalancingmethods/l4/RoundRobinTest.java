package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

class RoundRobinTest {

    @Test
    void getBackendAddress() {
        List<InetSocketAddress> addressList = new ArrayList<>();

        // Add Backend Server Addresses
        for (int i = 1; i <= 100; i++) {
            addressList.add(new InetSocketAddress("192.168.1." + i, i));
        }

        L4Balancer l4Balancer = new RoundRobin(addressList);

        for (int i = 1; i <= 100; i++) {
            assertEquals(new InetSocketAddress("192.168.1." + i, i), l4Balancer.getBackendAddress(null));
        }

        for (int i = 1; i <= 100; i++) {
            assertEquals(new InetSocketAddress("192.168.1." + i, i), l4Balancer.getBackendAddress(null));
        }

        for (int i = 1; i <= 100; i++) {
            assertNotEquals(new InetSocketAddress("10.10.1." + i, i), l4Balancer.getBackendAddress(null));
        }

        for (int i = 1; i <= 100; i++) {
            assertNotEquals(new InetSocketAddress("172.16.20." + i, i), l4Balancer.getBackendAddress(null));
        }
    }
}
