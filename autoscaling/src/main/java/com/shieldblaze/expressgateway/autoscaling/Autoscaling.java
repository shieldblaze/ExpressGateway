/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.autoscaling;

import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingScaleDownEvent;
import com.shieldblaze.expressgateway.autoscaling.event.AutoscalingScaleUpEvent;
import com.shieldblaze.expressgateway.concurrent.eventstream.EventStream;

import java.util.List;

public abstract class Autoscaling<T> {

    private final EventStream eventStream;

    /**
     * Create a new {@link Autoscaling<T>} Instance
     *
     * @param eventStream {@link EventStream} where events will be published
     */
    public Autoscaling(EventStream eventStream) {
        this.eventStream = eventStream;
    }

    /**
     * Scale up servers fleet by 1 (one).
     */
    public abstract AutoscalingScaleUpEvent scaleUp();

    /**
     * Scale down servers fleet by 1 (one).
     */
    public abstract AutoscalingScaleDownEvent scaleDown();

    /**
     * List of Servers
     */
    public abstract List<T> servers();

    public EventStream eventStream() {
        return eventStream;
    }
}
