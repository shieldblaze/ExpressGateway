package com.shieldblaze.expressgateway.loadbalance.l4;

import com.shieldblaze.expressgateway.loadbalance.backend.Backend;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class NATForwardTest {

    @Test
    void getBackend() {
        List<Backend> addressList = new ArrayList<>();
        addressList.add(fastBuild("192.168.1.1"));

        L4Balance l4Balance = new NATForward(addressList);

        assertEquals(fastBuild("192.168.1.1").getSocketAddress(), l4Balance.getBackend(new InetSocketAddress("10.10.10.1", 1)).getSocketAddress());
        assertEquals(fastBuild("192.168.1.1").getSocketAddress(), l4Balance.getBackend(new InetSocketAddress("10.10.10.2", 2)).getSocketAddress());
        assertEquals(fastBuild("192.168.1.1").getSocketAddress(), l4Balance.getBackend(new InetSocketAddress("10.10.10.3", 3)).getSocketAddress());
        assertEquals(fastBuild("192.168.1.1").getSocketAddress(), l4Balance.getBackend(new InetSocketAddress("10.10.10.4", 4)).getSocketAddress());
    }

    @Test
    void throwException() {
        List<Backend> addressList = new ArrayList<>();
        addressList.add(fastBuild("192.168.1.1"));
        addressList.add(fastBuild("192.168.1.2"));

        assertThrows(IllegalArgumentException.class, () -> new NATForward(addressList));
    }

    private Backend fastBuild(String host) {
        return new Backend(new InetSocketAddress(host, 1));
    }
}
