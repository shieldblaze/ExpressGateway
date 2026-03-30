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
package com.shieldblaze.expressgateway.controlplane.conflict;

import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class VectorClockTest {

    @Test
    void emptyClockHasNoEntries() {
        VectorClock clock = VectorClock.empty();
        assertTrue(clock.entries().isEmpty());
        assertEquals(0L, clock.get("any-instance"));
    }

    @Test
    void incrementCreatesNewEntryIfAbsent() {
        VectorClock clock = VectorClock.empty().increment("node-1");
        assertEquals(1L, clock.get("node-1"));
        assertEquals(0L, clock.get("node-2"));
    }

    @Test
    void incrementsAreAdditive() {
        VectorClock clock = VectorClock.empty()
                .increment("node-1")
                .increment("node-1")
                .increment("node-1");
        assertEquals(3L, clock.get("node-1"));
    }

    @Test
    void multipleNodesTrackedIndependently() {
        VectorClock clock = VectorClock.empty()
                .increment("node-1")
                .increment("node-2")
                .increment("node-1");
        assertEquals(2L, clock.get("node-1"));
        assertEquals(1L, clock.get("node-2"));
    }

    @Test
    void equalClocksCompareAsEqual() {
        VectorClock a = VectorClock.empty().increment("n1").increment("n2");
        VectorClock b = VectorClock.empty().increment("n1").increment("n2");
        assertEquals(VectorClock.Comparison.EQUAL, a.compareTo(b));
        assertEquals(VectorClock.Comparison.EQUAL, b.compareTo(a));
    }

    @Test
    void clockBeforeOther() {
        VectorClock a = VectorClock.empty().increment("n1");
        VectorClock b = VectorClock.empty().increment("n1").increment("n1");
        assertEquals(VectorClock.Comparison.BEFORE, a.compareTo(b));
        assertEquals(VectorClock.Comparison.AFTER, b.compareTo(a));
    }

    @Test
    void concurrentClocks() {
        // a has n1=2, n2=0
        VectorClock a = VectorClock.empty().increment("n1").increment("n1");
        // b has n1=0, n2=2
        VectorClock b = VectorClock.empty().increment("n2").increment("n2");
        assertEquals(VectorClock.Comparison.CONCURRENT, a.compareTo(b));
        assertEquals(VectorClock.Comparison.CONCURRENT, b.compareTo(a));
    }

    @Test
    void mergeMaxTakesComponentWiseMax() {
        VectorClock a = new VectorClock(Map.of("n1", 3L, "n2", 1L));
        VectorClock b = new VectorClock(Map.of("n1", 1L, "n2", 4L, "n3", 2L));

        VectorClock merged = a.merge(b);
        assertEquals(3L, merged.get("n1"));
        assertEquals(4L, merged.get("n2"));
        assertEquals(2L, merged.get("n3"));
    }

    @Test
    void mergedClockIsAfterBothInputs() {
        VectorClock a = new VectorClock(Map.of("n1", 3L, "n2", 1L));
        VectorClock b = new VectorClock(Map.of("n1", 1L, "n2", 4L));
        VectorClock merged = a.merge(b);

        assertEquals(VectorClock.Comparison.AFTER, merged.compareTo(a));
        assertEquals(VectorClock.Comparison.AFTER, merged.compareTo(b));
    }

    @Test
    void immutabilityGuaranteed() {
        VectorClock original = VectorClock.empty().increment("n1");
        VectorClock incremented = original.increment("n1");

        assertEquals(1L, original.get("n1"));
        assertEquals(2L, incremented.get("n1"));
    }

    @Test
    void serializationRoundTrip() throws Exception {
        ObjectMapper mapper = new ObjectMapper();
        VectorClock clock = new VectorClock(Map.of("n1", 5L, "n2", 3L));

        String json = mapper.writeValueAsString(clock);
        VectorClock deserialized = mapper.readValue(json, VectorClock.class);

        assertNotNull(deserialized);
        assertEquals(5L, deserialized.get("n1"));
        assertEquals(3L, deserialized.get("n2"));
        assertEquals(VectorClock.Comparison.EQUAL, clock.compareTo(deserialized));
    }

    @Test
    void emptyClocksAreEqual() {
        assertEquals(VectorClock.Comparison.EQUAL,
                VectorClock.empty().compareTo(VectorClock.empty()));
    }

    @Test
    void emptyClockIsBeforeAnyNonEmpty() {
        VectorClock empty = VectorClock.empty();
        VectorClock nonEmpty = VectorClock.empty().increment("n1");
        assertEquals(VectorClock.Comparison.BEFORE, empty.compareTo(nonEmpty));
        assertEquals(VectorClock.Comparison.AFTER, nonEmpty.compareTo(empty));
    }

    @Test
    void negativeClockValueRejected() {
        assertThrows(IllegalArgumentException.class,
                () -> new VectorClock(Map.of("n1", -1L)));
    }

    @Test
    void negativeClockValueRejectedInDeserialization() {
        ObjectMapper mapper = new ObjectMapper();
        // JSON with a negative clock value
        String json = "{\"entries\":{\"n1\":-5}}";
        assertThrows(Exception.class,
                () -> mapper.readValue(json, VectorClock.class));
    }

    @Test
    void zeroClockValueAccepted() {
        VectorClock clock = new VectorClock(Map.of("n1", 0L));
        assertEquals(0L, clock.get("n1"));
    }
}
