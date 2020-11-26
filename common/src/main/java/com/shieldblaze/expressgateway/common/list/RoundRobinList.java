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

import com.shieldblaze.expressgateway.common.algo.roundrobin.RoundRobinIndexGenerator;

import java.util.Collections;
import java.util.List;
import java.util.Objects;

/**
 * Round-Robin List Implementation
 */
public final class RoundRobinList<T> {
    private List<T> list;
    private RoundRobinIndexGenerator roundRobinIndexGenerator;

    public RoundRobinList() {
        this(Collections.emptyList());
    }

    public RoundRobinList(List<T> list) {
        init(list);
    }

    public T next() throws Exception {
        return list.get(roundRobinIndexGenerator.next());
    }

    public void init(List<T> list) {
        Objects.requireNonNull(list, "List");
        this.list = list;
        roundRobinIndexGenerator = new RoundRobinIndexGenerator(list);
    }

    public RoundRobinIndexGenerator roundRobinIndexGenerator() {
        return roundRobinIndexGenerator;
    }
}
