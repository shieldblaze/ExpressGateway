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
package com.shieldblaze.expressgateway.concurrent.eventstream;

import com.shieldblaze.expressgateway.concurrent.event.Event;

/**
 * {@linkplain EventSubscriber} exposes {@link EventStream#publish(Event)}
 * via {@link #subscribe(EventListener)}.
 */
public final class EventSubscriber {

    private final EventStream eventStream;

    EventSubscriber(EventStream eventStream) {
        this.eventStream = eventStream;
    }

    public <T> EventSubscriber subscribe(EventListener<T> eventListener) {
        eventStream.subscribe(eventListener);
        return this;
    }

    public <T> EventSubscriber unsubscribe(EventListener<T> eventListener) {
        eventStream.unsubscribe(eventListener);
        return this;
    }
}
