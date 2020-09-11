package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import com.shieldblaze.expressgateway.backend.Backend;

import java.net.InetSocketAddress;
import java.util.List;

public final class WeightedRoundRobin extends L4Balance {

    private final RoundRobinListImpl<Backend> backendsRoundRobin;

    public WeightedRoundRobin(List<Backend> socketAddressList) {
        super(socketAddressList);
        getBackends().sort((b1, b2) -> Integer.compare(b1.getWeight(), b2.getConnections()));
        backendsRoundRobin = new RoundRobinListImpl<>(getBackends());
    }

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        return backendsRoundRobin.iterator().next();
    }
}
