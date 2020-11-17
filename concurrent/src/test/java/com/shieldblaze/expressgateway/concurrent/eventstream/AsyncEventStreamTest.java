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
package com.shieldblaze.expressgateway.concurrent.eventstream;

import com.shieldblaze.expressgateway.concurrent.event.Event;
import org.junit.jupiter.api.Test;

import java.util.concurrent.ConcurrentSkipListSet;
import java.util.concurrent.Executors;

import static org.junit.jupiter.api.Assertions.assertTrue;

class AsyncEventStreamTest {

    @Test
    void testAsyncEventStream() {
        EventListenerTest eventListenerTest = new EventListenerTest();

        AsyncEventStream asyncEventStream = new AsyncEventStream(Executors.newCachedThreadPool());
        asyncEventStream.subscribe(eventListenerTest);

        for (int i = 0; i < 100_000; i++) {
            asyncEventStream.publish(new SimpleEvent("Meow" + i));
        }

        assertTrue(asyncEventStream.unsubscribe(eventListenerTest));
    }

    private static final class EventListenerTest implements EventListener {

        final ConcurrentSkipListSet<String> stringsSet = new ConcurrentSkipListSet<>();

        public EventListenerTest() {
            for (int i = 0; i < 100_000; i++) {
                stringsSet.add("Meow" + i);
            }
        }

        @Override
        public void accept(Event event) {
            assertTrue(stringsSet.contains(((SimpleEvent) event).string));
        }
    }

    private static final class SimpleEvent implements Event {
        private final String string;

        public SimpleEvent(String string) {
            this.string = string;
        }
    }
}
