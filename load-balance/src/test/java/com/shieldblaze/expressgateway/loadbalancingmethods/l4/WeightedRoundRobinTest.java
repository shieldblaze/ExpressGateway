package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import com.shieldblaze.expressgateway.backend.Backend;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

class WeightedRoundRobinTest {

    @Test
    void getBackend() {

        List<Backend> backends = new ArrayList<>();
        backends.add(fastBuild("10.10.1.1", 30));
        backends.add(fastBuild("10.10.1.2", 20));
        backends.add(fastBuild("10.10.1.3", 40));
        backends.add(fastBuild("10.10.1.4", 10));

        int first = 0;
        int second = 0;
        int third = 0;
        int forth = 0;

        L4Balance l4Balance = new WeightedRoundRobin(backends);

        for (int i = 0; i < 100_000; i++) {
            switch (l4Balance.getBackend(null).getInetSocketAddress().getHostString()) {
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

        assertTrue(first > 25000);
        assertTrue(second > 15000);
        assertTrue(third > 35000);
        assertTrue(forth > 5000);
    }

    private Backend fastBuild(String host, int weight) {
        return new Backend(new InetSocketAddress(host, 1), weight, 0);
    }
}
