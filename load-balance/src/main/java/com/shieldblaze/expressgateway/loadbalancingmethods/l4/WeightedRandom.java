package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import com.google.common.collect.Range;
import com.google.common.collect.TreeRangeMap;
import com.shieldblaze.expressgateway.backend.Backend;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.TreeMap;

public final class WeightedRandom extends L4Balance {
    private static final java.util.Random RANDOM_INSTANCE = new java.util.Random();

    private final TreeRangeMap<Integer, Backend> backends = TreeRangeMap.create();
    private int totalWeight = 0;

    public WeightedRandom(List<Backend> backends) {
        super(backends);
        backends.forEach(backend -> this.backends.put(Range.closed(totalWeight,  totalWeight += backend.getWeight()), backend));
        getBackends().clear();
    }

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        int index = RANDOM_INSTANCE.nextInt(totalWeight);
        return backends.get(index);
    }
}
