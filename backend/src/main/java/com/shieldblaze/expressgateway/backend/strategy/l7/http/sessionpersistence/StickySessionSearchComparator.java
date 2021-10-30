/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence;

import com.shieldblaze.expressgateway.backend.Node;

import java.util.Comparator;

final class StickySessionSearchComparator implements Comparator<Object> {

    static final StickySessionSearchComparator INSTANCE = new StickySessionSearchComparator();

    private StickySessionSearchComparator() {
        // Prevent outside initialization
    }

    @Override
    public int compare(Object o1, Object o2) {
        String key = (String) o1;
        Node node = (Node) o2;
        return node.id().compareToIgnoreCase(key);
    }
}
