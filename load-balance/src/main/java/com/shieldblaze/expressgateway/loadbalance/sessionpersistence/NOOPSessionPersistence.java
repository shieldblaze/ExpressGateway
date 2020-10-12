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
 */        // Does Nothing
package com.shieldblaze.expressgateway.loadbalance.sessionpersistence;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.loadbalance.l7.Request;
import io.netty.handler.codec.http.HttpRequest;

import java.net.InetSocketAddress;

/**
 * No-Operation {@link SessionPersistence}
 */
public final class NOOPSessionPersistence extends SessionPersistence {

    @Override
    public Backend getBackend(InetSocketAddress sourceAddress) {
        return null;
    }

    @Override
    public Backend getBackend(Request request) {
        return null;
    }

    @Override
    public void addRoute(InetSocketAddress socketAddress, Backend backend) {
        // Does Nothing
    }

    @Override
    public void addRoute(Request request, Backend backend) {
        // Does Nothing
    }
}
