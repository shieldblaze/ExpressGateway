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
import org.junit.jupiter.api.Test;

import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
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

        final Set<String> stringsSet = new HashSet<>();

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

    private record SimpleEvent(String string) implements Event<Void> {

        @Override
            public CompletableFuture<Void> future() {
                return null;
            }

            @Override
            public boolean isFinished() {
                return false;
            }

            @Override
            public boolean isSuccess() {
                return false;
            }

            @Override
            public Throwable cause() {
                return null;
            }
        }
}
