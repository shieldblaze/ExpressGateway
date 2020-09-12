package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import com.shieldblaze.expressgateway.backend.Backend;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

class WeightedLeastConnectionTest {

    @Test
    void getBackend() {
        List<Backend> backends = new ArrayList<>();
        backends.add(fastBuild("10.10.1.1", 10, 0));
        backends.add(fastBuild("10.10.1.2", 20, 0));
        backends.add(fastBuild("10.10.1.3", 30, 0));
        backends.add(fastBuild("10.10.1.4", 40, 0));

        int first = 0;
        int second = 0;
        int third = 0;
        int forth = 0;

        L4Balance l4Balance = new WeightedLeastConnection(backends);

        for (int i = 0; i < 1000000; i++) {
            Backend backend = l4Balance.getBackend(null);
            backend.incConnections();
            switch (backend.getInetSocketAddress().getHostString()) {
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

        assertEquals(100000, first);
        assertEquals(200000, second);
        assertEquals(300000, third);
        assertEquals(400000, forth);
    }

    private static Backend fastBuild(String host, int weight, int connection) {
        return new Backend(new InetSocketAddress(host, 1), weight, connection);
    }
}
