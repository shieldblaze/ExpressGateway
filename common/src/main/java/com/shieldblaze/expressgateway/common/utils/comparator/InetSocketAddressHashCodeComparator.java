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
package com.shieldblaze.expressgateway.common.utils.comparator;

import java.net.InetSocketAddress;
import java.util.Comparator;

/**
 * {@link InetSocketAddress} comparator using hash code.
 */
public final class InetSocketAddressHashCodeComparator implements Comparator<InetSocketAddress> {

    public static final InetSocketAddressHashCodeComparator INSTANCE = new InetSocketAddressHashCodeComparator();

    private InetSocketAddressHashCodeComparator() {
        // Prevent outside initialization
    }

    @Override
    public int compare(InetSocketAddress o1, InetSocketAddress o2) {
        return Integer.compare(o1.hashCode(), o2.hashCode());
    }
}
