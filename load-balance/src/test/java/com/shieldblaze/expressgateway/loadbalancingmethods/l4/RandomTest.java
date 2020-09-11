package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import com.shieldblaze.expressgateway.backend.Backend;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.*;

class RandomTest {

    @Test
    void getBackend() {
        List<Backend> addressList = new ArrayList<>();

        // Add Backend Server Addresses
        addressList.add(fastBuild("172.16.20.1"));
        addressList.add(fastBuild("172.16.20.2"));
        addressList.add(fastBuild("172.16.20.3"));
        addressList.add(fastBuild("172.16.20.4"));
        addressList.add(fastBuild("172.16.20.5"));

        L4Balance l4Balance = new Random(addressList);

        int first = 0;
        int second = 0;
        int third = 0;
        int forth = 0;
        int fifth = 0;

        for (int i = 0; i < 1000; i++) {
            switch (l4Balance.getBackend(null).getInetSocketAddress().getHostString()) {
                case "172.16.20.1": {
                    first++;
                    break;
                }
                case "172.16.20.2": {
                    second++;
                    break;
                }
                case "172.16.20.3": {
                    third++;
                    break;
                }
                case "172.16.20.4": {
                    forth++;
                    break;
                }
                case "172.16.20.5": {
                    fifth++;
                    break;
                }
                default:
                    break;
            }
        }

        assertTrue(first > 10);
        assertTrue(second > 10);
        assertTrue(third > 10);
        assertTrue(forth > 10);
        assertTrue(fifth > 10);
    }

    private Backend fastBuild(String host) {
        return new Backend(new InetSocketAddress(host, 1), 1, 0);
    }
}