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
package com.shieldblaze.expressgateway.bootstrap;

import org.junit.jupiter.api.Test;

import java.time.Duration;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class StartupMetricsTest {

    @Test
    void initialState() {
        StartupMetrics metrics = new StartupMetrics();
        assertFalse(metrics.isReady());
        assertEquals(Duration.ZERO, metrics.timeToReady());
        assertTrue(metrics.componentTimes().isEmpty());
    }

    @Test
    void timeComponent() throws Exception {
        StartupMetrics metrics = new StartupMetrics();
        metrics.timeComponent("test-component", () -> Thread.sleep(10));

        assertTrue(metrics.componentTimes().containsKey("test-component"));
        assertTrue(metrics.componentTimes().get("test-component").toMillis() >= 10);
    }

    @Test
    void failedComponentRecordsDuration() {
        StartupMetrics metrics = new StartupMetrics();
        assertThrows(RuntimeException.class,
                () -> metrics.timeComponent("failing", () -> { throw new RuntimeException("fail"); }));

        assertTrue(metrics.componentTimes().containsKey("failing (FAILED)"));
    }

    @Test
    void startupLifecycle() throws Exception {
        StartupMetrics metrics = new StartupMetrics();
        metrics.markStartupBegin();

        metrics.timeComponent("comp-a", () -> Thread.sleep(5));
        metrics.timeComponent("comp-b", () -> Thread.sleep(5));

        metrics.markStartupComplete();

        assertTrue(metrics.isReady());
        assertTrue(metrics.timeToReady().toMillis() >= 10);
        assertEquals(2, metrics.componentTimes().size());
    }

    @Test
    void componentTimesAreImmutableCopy() throws Exception {
        StartupMetrics metrics = new StartupMetrics();
        metrics.timeComponent("comp-a", () -> {});

        var times = metrics.componentTimes();
        assertNotNull(times);
        assertThrows(UnsupportedOperationException.class,
                () -> times.put("comp-b", Duration.ZERO));
    }
}
