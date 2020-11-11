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
package com.shieldblaze.expressgateway.common.list;

import javax.annotation.Nonnull;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Iterator;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Round-Robin List Implementation
 */
public final class RoundRobinList<T> implements Iterable<T> {
    private List<T> list;
    private Iterator<T> iterator;
    private final AtomicInteger index = new AtomicInteger(0);

    public RoundRobinList() {
        this(Collections.emptyList());
    }

    public RoundRobinList(List<T> list) {
        newIterator(list);
    }

    @Nonnull
    @Override
    public Iterator<T> iterator() {
        return iterator;
    }

    public void newIterator(List<T> list) {
        this.list = new ArrayList<>(Objects.requireNonNull(list, "list"));

        index.set(0);
        this.iterator = new Iterator<>() {
            @Override
            public boolean hasNext() {
                // Because it's round-robin
                return true;
            }

            @Override
            public T next() {
                index.compareAndSet(list.size(), 0);
                return list.get(index.getAndIncrement());
            }

            @Override
            public void remove() {
                // We don't support removal of elements
                // because we're just a Round-Robin Iterator.
                throw new UnsupportedOperationException();
            }
        };
    }

    public List<T> getList() {
        return list;
    }
}
