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
package com.shieldblaze.expressgateway.metrics;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

class ThroughputMeterTest {

    @Test
    void totalCountTracking() {
        ThroughputMeter meter = new ThroughputMeter();

        meter.mark();
        meter.mark();
        meter.mark();

        assertEquals(3, meter.totalCount());
    }

    @Test
    void totalBytesTracking() {
        ThroughputMeter meter = new ThroughputMeter();

        meter.markBytes(100);
        meter.markBytes(200);
        meter.markBytes(300);

        assertEquals(3, meter.totalCount());
        assertEquals(600, meter.totalBytes());
    }

    @Test
    void rateIsPositiveAfterRecording() {
        ThroughputMeter meter = new ThroughputMeter();

        for (int i = 0; i < 100; i++) {
            meter.mark();
        }

        // Rate should be positive (we just recorded 100 events)
        double rate = meter.rate();
        assertTrue(rate > 0, "Rate should be positive after recording, was: " + rate);
    }

    @Test
    void byteRateIsPositiveAfterRecording() {
        ThroughputMeter meter = new ThroughputMeter();

        for (int i = 0; i < 100; i++) {
            meter.markBytes(1024);
        }

        double byteRate = meter.byteRate();
        assertTrue(byteRate > 0, "Byte rate should be positive, was: " + byteRate);
    }

    @Test
    void bulkMark() {
        ThroughputMeter meter = new ThroughputMeter();

        meter.mark(10, 5000);

        assertEquals(10, meter.totalCount());
        assertEquals(5000, meter.totalBytes());
    }

    @Test
    void invalidIntervalThrows() {
        assertThrows(IllegalArgumentException.class, () -> new ThroughputMeter(0));
        assertThrows(IllegalArgumentException.class, () -> new ThroughputMeter(-1));
    }

    @Test
    void customInterval() {
        // 5-second interval
        ThroughputMeter meter = new ThroughputMeter(5_000_000_000L);

        meter.mark();
        meter.mark();

        assertEquals(2, meter.totalCount());
        assertTrue(meter.rate() >= 0);
    }
}
