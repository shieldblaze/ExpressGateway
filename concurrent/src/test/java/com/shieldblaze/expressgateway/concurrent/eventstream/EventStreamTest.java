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

import org.junit.jupiter.api.Assertions;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertTrue;

class EventStreamTest {

    @Test
    void testEventStream() {
        EventListenerTest eventListenerTest = new EventListenerTest();

        EventStream eventStream = new EventStream();
        eventStream.subscribe(eventListenerTest);

        for (int i = 0; i < 100_000; i++) {
            eventStream.publish("Meow" + i);
        }

        assertTrue(eventStream.unsubscribe(eventListenerTest));
    }

    private static final class EventListenerTest implements EventListener {

        int expect = 0;

        @Override
        public void accept(Object event) {
            Assertions.assertEquals("Meow" + expect, event);
            expect++;
        }
    }
}
