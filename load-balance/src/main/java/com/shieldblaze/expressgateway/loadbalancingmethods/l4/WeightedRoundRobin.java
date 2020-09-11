package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import com.shieldblaze.expressgateway.backend.Backend;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.TreeMap;

public final class WeightedRoundRobin extends L4Balance {
    private static final java.util.Random RANDOM_INSTANCE = new java.util.Random();

    private final TreeMap<Integer, Backend> backends;
    private int totalWeight;

    public WeightedRoundRobin(List<Backend> backends) {
        super(backends);

        this.backends = new TreeMap<>();
        totalWeight = 0;

        backends.forEach(backend -> {
            totalWeight += backend.getWeight();
            this.backends.put(totalWeight, backend);
        });

        getBackends().clear(); // We don't need this list anymore
    }

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        int rnd = RANDOM_INSTANCE.nextInt(this.totalWeight);
        return backends.ceilingEntry(rnd).getValue();
    }
}
