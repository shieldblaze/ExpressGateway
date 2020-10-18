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
package com.shieldblaze.expressgateway.backend.connection;

import io.netty.channel.ChannelFuture;

/**
 * Connection to a backend
 */
public class Connection {

    protected ChannelFuture channelFuture;
    private boolean inUse;

    /**
     * Check if connection is active and connected.
     *
     * @return {@code true} if connection is active and connected else {@code false}
     */
    public boolean isActive() {
        return channelFuture.channel().isActive();
    }

    /**
     * Check if connection is in use.
     *
     * @return {@code true} if connection is in use else {@code false}
     */
    public boolean isInUse() {
        return inUse;
    }

    /**
     * Set {@code inUse} property
     *
     * @param inUse Set to {@code true} if connection is in use and should not be acquired
     *              else set to {@code false}
     */
    public void setInUse(boolean inUse) {
        this.inUse = inUse;
    }
}
