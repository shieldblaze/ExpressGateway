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
package com.shieldblaze.expressgateway.core.events;

import com.shieldblaze.expressgateway.common.internal.Internal;
import com.shieldblaze.expressgateway.concurrent.events.Event;
import com.shieldblaze.expressgateway.core.server.L4FrontListener;
import io.netty.channel.ChannelFuture;

/**
 * {@link Event} for {@link L4FrontListener}
 */
public final class L4FrontListenerEvent implements Event {
    private boolean isValueSet;

    private boolean isSuccess;
    private Throwable cause;
    private ChannelFuture channelFuture;

    @Override
    public boolean isSuccess() {
        return isSuccess;
    }

    @Override
    public Throwable cause() {
        return cause;
    }

    @Internal
    public void setCause(Throwable cause) {
        if (isValueSet) {
            throw new IllegalArgumentException("Value already set");
        }
        this.cause = cause;
        isValueSet = true;
    }

    @Internal
    public ChannelFuture getChannelFuture() {
        return channelFuture;
    }

    @Internal
    public void setChannelFuture(ChannelFuture channelFuture) {
        if (isValueSet) {
            throw new IllegalArgumentException("Value already set");
        }
        this.channelFuture = channelFuture;
        this.isSuccess = true;
    }
}
