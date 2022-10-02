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
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.Closeable;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

/**
 * {@linkplain EventStream} is a simple pub-sub stream channel.
 */
public class EventStream implements Closeable {

    private static final Logger logger = LogManager.getLogger(EventStream.class);

    /**
     * List of subscribers
     */
    protected final List<EventListener> subscribers = new CopyOnWriteArrayList<>();

    /**
     * Subscribe to this {@linkplain EventStream}
     *
     * @param eventListener Class implementing {@link EventListener} who will subscribe
     */
    public void subscribe(EventListener eventListener) {
        subscribers.add(eventListener);
    }

    /**
     * Unsubscribe from this {@linkplain EventStream}
     *
     * @param eventListener Class implementing {@link EventListener} who wants to unsubscribe
     * @return Returns {@code true} if unsubscribe was successful else {@code false}
     */
    public boolean unsubscribe(EventListener eventListener) {
        return subscribers.remove(eventListener);
    }

    /**
     * Unsubscribe all subscribed {@linkplain EventListener} from this {@linkplain EventStream}
     */
    public void unsubscribeAll() {
        subscribers.clear();
    }

    /**
     * Publish an Event to all subscribed {@linkplain EventListener}
     *
     * @param event Event to publish
     */
    @SuppressWarnings("unchecked")
    public void publish(Event event) {
        subscribers.forEach(eventListener -> eventListener.accept(event));
    }

    /**
     * Copy all subscribers from other {@link EventStream} to specified {@link EventStream}
     */
    public void addSubscribersFrom(EventStream eventStream) {
        subscribers.addAll(eventStream.subscribers);
    }

    @Override
    public void close() {
        logger.info("Unsubscribing all subscribers. Subscribers size: {}", subscribers.size());

        // If Debug is enabled then log all subscribers
        if (logger.isDebugEnabled()) {
            logger.debug("Subscribers: {}", subscribers);
        }

        unsubscribeAll();
        logger.info("Subscribed all subscribers. Subscribers size: {}", subscribers.size());
    }

    @Override
    public String toString() {
        return "EventStream{subscribersSize=" + subscribers.size() + '}';
    }
}
