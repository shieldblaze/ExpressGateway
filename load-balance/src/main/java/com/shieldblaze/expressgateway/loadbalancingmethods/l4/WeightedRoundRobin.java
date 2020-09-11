package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import com.google.common.collect.Range;
import com.google.common.collect.TreeRangeMap;
import com.shieldblaze.expressgateway.backend.Backend;

import java.net.InetSocketAddress;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

public class WeightedRoundRobin extends L4Balance {

    private final AtomicInteger Index = new AtomicInteger();
    private final TreeRangeMap<Integer, Backend> backends = TreeRangeMap.create();
    private int totalWeight = 0;

    public WeightedRoundRobin(List<Backend> backends) {
        super(backends);
        backends.forEach(backend -> this.backends.put(Range.closed(totalWeight, totalWeight += backend.getWeight()), backend));
        getBackends().clear();
    }

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        if (Index.get() > totalWeight) {
            Index.set(0);
        }
        return backends.get(Index.getAndIncrement());
    }
}
