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
package com.shieldblaze.expressgateway.common.list;

import org.junit.jupiter.api.Test;

import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;

class RoundRobinListTest {

    @Test
    public void threadSafetyTest() throws Exception {

        List<Integer> integers = new ArrayList<>();
        for (int i = 0; i < 100_000; i++) {
            integers.add(i);
        }

        RoundRobinList<Integer> roundRobinList = new RoundRobinList<>(integers);

        Thread t1 = new Thread(() -> {
            for (int j = 0; j < 10_000_000; j++) {
                assertDoesNotThrow(roundRobinList::next);
            }
        });

        Thread t2 = new Thread(() -> {
            for (int j = 0; j < 10_000_000; j++) {
                assertDoesNotThrow(roundRobinList::next);
            }
        });

        Thread t3 = new Thread(() -> {
            for (int j = 0; j < 10_000_000; j++) {
                assertDoesNotThrow(roundRobinList::next);
            }
        });

        Thread t4 = new Thread(() -> {
            for (int j = 0; j < 10_000_000; j++) {
                assertDoesNotThrow(roundRobinList::next);
            }
        });

        Thread t5 = new Thread(() -> {
            for (int j = 0; j < 10_000_000; j++) {
                assertDoesNotThrow(roundRobinList::next);
            }
        });

        t1.start();
        t2.start();
        t3.start();
        t4.start();
        t5.start();

        t1.join();
        t2.join();
        t3.join();
        t4.join();
        t5.join();
    }
}
