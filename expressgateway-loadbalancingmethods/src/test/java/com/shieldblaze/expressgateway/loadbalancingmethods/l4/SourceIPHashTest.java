package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

class SourceIPHashTest {

    @Test
    void getBackendAddress() {
        List<InetSocketAddress> addressList = new ArrayList<>();

        // Add Backend Server Addresses
        addressList.add(new InetSocketAddress("172.16.20.1", 9110));
        addressList.add(new InetSocketAddress("172.16.20.2", 9110));

        L4Balancer l4Balancer = new SourceIPHash(addressList);
        assertEquals(new InetSocketAddress("172.16.20.1", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("192.168.1.1", 1)));
        assertEquals(new InetSocketAddress("172.16.20.1", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("192.168.1.23", 1)));
        assertEquals(new InetSocketAddress("172.16.20.1", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("192.168.1.84", 1)));
        assertEquals(new InetSocketAddress("172.16.20.1", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("192.168.1.251", 1)));

        assertEquals(new InetSocketAddress("172.16.20.2", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("10.18.1.10", 1)));
        assertEquals(new InetSocketAddress("172.16.20.2", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("10.18.1.43", 1)));
        assertEquals(new InetSocketAddress("172.16.20.2", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("10.18.1.72", 1)));
        assertEquals(new InetSocketAddress("172.16.20.2", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("10.18.1.213", 1)));

        assertEquals(new InetSocketAddress("172.16.20.1", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("127.0.0.10", 1)));
        assertEquals(new InetSocketAddress("172.16.20.1", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("127.0.0.172", 1)));
        assertEquals(new InetSocketAddress("172.16.20.1", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("127.0.0.230", 1)));
        assertEquals(new InetSocketAddress("172.16.20.1", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("127.0.0.253", 1)));

        assertEquals(new InetSocketAddress("172.16.20.2", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("172.20.1.10", 1)));
        assertEquals(new InetSocketAddress("172.16.20.2", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("172.20.1.172", 1)));
        assertEquals(new InetSocketAddress("172.16.20.2", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("172.20.1.230", 1)));
        assertEquals(new InetSocketAddress("172.16.20.2", 9110), l4Balancer.getBackendAddress(new InetSocketAddress("172.20.1.253", 1)));
    }
}
