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
package com.shieldblaze.expressgateway.loadbalance.l4.sessionpersistence;

import com.shieldblaze.expressgateway.loadbalance.backend.Backend;

import java.net.InetSocketAddress;

/**
 * Session Persistence routes connection to specific {@link Backend}
 */
public abstract class SessionPersistence {

    /**
     * Get {@link Backend}
     * @return {@link Backend} is route is available else {@code null}
     */
    public abstract Backend getBackend(InetSocketAddress sourceAddress);

    /**
     * Add route to {@link Backend}
     */
    public abstract void addRoute(InetSocketAddress socketAddress, Backend backend);
}
