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
package com.shieldblaze.expressgateway.loadbalance.l4;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;

import java.net.InetSocketAddress;

/**
 * No-Operation {@link SessionPersistence}
 */
final class NOOPSessionPersistence implements SessionPersistence<Backend, Backend, InetSocketAddress, Backend> {

    @Override
    public Backend getBackend(Request request) {
        return null;
    }

    @Override
    public Backend addRoute(InetSocketAddress socketAddress, Backend backend) {
        return null;
    }

    @Override
    public boolean removeRoute(InetSocketAddress inetSocketAddress, Backend backend) {
        return false;
    }

    @Override
    public void clear() {
        // Does nothing
    }
}
