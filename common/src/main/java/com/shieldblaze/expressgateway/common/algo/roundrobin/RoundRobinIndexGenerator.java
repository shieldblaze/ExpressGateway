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

import java.util.Collection;
import java.util.concurrent.atomic.AtomicInteger;

public class RoundRobinIndexGenerator {

    private final AtomicInteger atomicInteger = new AtomicInteger(-1);
    private final AtomicInteger maxIndex;

    public RoundRobinIndexGenerator(Collection<?> collection) {
        this(collection.size());
    }

    public RoundRobinIndexGenerator(int maxIndex) {
        this.maxIndex = new AtomicInteger(maxIndex);
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

        try {
            return nextIndex % maxIndex.get();
        } catch (Exception ex) {
            return -1;
        }
    }

    public void incMaxIndex() {
        maxIndex.addAndGet(1);
        atomicInteger.set(-1);
    }

    public void decMaxIndex() {
        maxIndex.updateAndGet(operand -> operand - 1);
        atomicInteger.set(-1);
    }

    public void setMaxIndex(int maxIndex) {
        this.maxIndex.set(maxIndex);
        atomicInteger.set(-1);
    }

    public AtomicInteger maxIndex() {
        return maxIndex;
    }
}
