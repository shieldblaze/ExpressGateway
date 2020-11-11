/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.common.algo.roundrobin;

import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicInteger;

public class RoundRobinIndexGenerator {

    private final AtomicInteger atomicInteger = new AtomicInteger(-1);
    private final int maxIndex;

    public RoundRobinIndexGenerator(List<?> list) {
        this(list.size());
    }

    public RoundRobinIndexGenerator(Map<?, ?> map) {
        this(map.size());
    }

    public RoundRobinIndexGenerator(int maxIndex) {
        this.maxIndex = maxIndex;
    }

    /**
     * Get the next index of Round-Robin
     */
    public int next() {
        int currentIndex;
        int nextIndex;

        do {
            currentIndex = atomicInteger.get();
            nextIndex = currentIndex < Integer.MAX_VALUE ? currentIndex + 1 : 0;
        } while (!atomicInteger.compareAndSet(currentIndex, nextIndex));

        return nextIndex % maxIndex;
    }
}
