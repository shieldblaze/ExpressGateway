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
package com.shieldblaze.expressgateway.loadbalancingmethods.l4;

import java.util.Iterator;
import java.util.List;

final class RoundRobinImpl<T> implements Iterable<T> {
    private final List<T> list;
    private int index = 0;

    RoundRobinImpl(List<T> list) { this.list = list; }

    public Iterator<T> iterator() {

        return new Iterator<T>() {

            @Override
            public boolean hasNext() {
                // Because it's round-robin
                return true;
            }

            @Override
            public T next() {
                try {
                    // If Index size has reached List size, set it to zero.
                    // Else, get the element from list as per latest Index count.
                    if(index == list.size()) {
                        index = 0;
                    }
                    return list.get(index);
                } finally {
                    index++;
                }
            }

            @Override
            public void remove() {
                // We don't support removal of elements
                throw new UnsupportedOperationException();
            }
        };
    }

    public int getIndex() {
        return index;
    }
}
