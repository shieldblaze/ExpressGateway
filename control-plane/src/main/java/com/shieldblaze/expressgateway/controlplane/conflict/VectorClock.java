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

import com.fasterxml.jackson.annotation.JsonCreator;
import com.fasterxml.jackson.annotation.JsonProperty;

import java.util.Collections;
import java.util.HashMap;
import java.util.Map;
import java.util.Objects;

/**
 * Immutable vector clock for causality tracking across distributed control plane instances.
 *
 * <p>Each control plane instance maintains its own logical clock counter within the vector.
 * Comparison of two vector clocks yields one of four relationships:</p>
 * <ul>
 *   <li>{@link Comparison#BEFORE} -- this clock happened before the other</li>
 *   <li>{@link Comparison#AFTER} -- this clock happened after the other</li>
 *   <li>{@link Comparison#EQUAL} -- both clocks represent the same event</li>
 *   <li>{@link Comparison#CONCURRENT} -- neither clock happened before the other (conflict)</li>
 * </ul>
 *
 * <p>Thread safety: this class is immutable. All mutating operations return new instances.</p>
 *
 * @param entries map from instance ID to its logical clock value; defensively copied
 */
public record VectorClock(@JsonProperty("entries") Map<String, Long> entries) {

    /**
     * Causality comparison result between two vector clocks.
     */
    public enum Comparison {
        BEFORE,
        AFTER,
        EQUAL,
        CONCURRENT
    }

    private static final VectorClock EMPTY = new VectorClock(Map.of());

    @JsonCreator
    public VectorClock {
        Objects.requireNonNull(entries, "entries");
        // Validate that all clock values are non-negative
        for (Map.Entry<String, Long> entry : entries.entrySet()) {
            if (entry.getValue() < 0) {
                throw new IllegalArgumentException(
                        "Clock value for instance '" + entry.getKey() + "' must be >= 0, got: " + entry.getValue());
            }
        }
        entries = Collections.unmodifiableMap(new HashMap<>(entries));
    }

    /**
     * Returns an empty vector clock.
     */
    public static VectorClock empty() {
        return EMPTY;
    }

    /**
     * Returns a new vector clock with the counter for the given instance incremented by one.
     *
     * @param instanceId the control plane instance ID to increment
     * @return a new vector clock with the incremented counter
     */
    public VectorClock increment(String instanceId) {
        Objects.requireNonNull(instanceId, "instanceId");
        Map<String, Long> newEntries = new HashMap<>(entries);
        newEntries.merge(instanceId, 1L, Long::sum);
        return new VectorClock(newEntries);
    }

    /**
     * Merges this vector clock with another, taking the max of each component.
     * The result represents a state that causally follows both input clocks.
     *
     * @param other the vector clock to merge with
     * @return a new vector clock representing the merged state
     */
    public VectorClock merge(VectorClock other) {
        Objects.requireNonNull(other, "other");
        Map<String, Long> merged = new HashMap<>(this.entries);
        for (Map.Entry<String, Long> entry : other.entries.entrySet()) {
            merged.merge(entry.getKey(), entry.getValue(), Math::max);
        }
        return new VectorClock(merged);
    }

    /**
     * Compares this vector clock to another for causality ordering.
     *
     * @param other the vector clock to compare against
     * @return the causal relationship between the two clocks
     */
    public Comparison compareTo(VectorClock other) {
        Objects.requireNonNull(other, "other");

        boolean thisBeforeOrEqual = true;  // all components of this <= other
        boolean otherBeforeOrEqual = true;  // all components of other <= this

        // Check all keys in both clocks
        var allKeys = new java.util.HashSet<>(this.entries.keySet());
        allKeys.addAll(other.entries.keySet());

        for (String key : allKeys) {
            long thisVal = this.entries.getOrDefault(key, 0L);
            long otherVal = other.entries.getOrDefault(key, 0L);

            if (thisVal > otherVal) {
                thisBeforeOrEqual = false;  // this has a component > other, so this is NOT <= other
            }
            if (thisVal < otherVal) {
                otherBeforeOrEqual = false; // other has a component > this, so other is NOT <= this
            }
        }

        if (thisBeforeOrEqual && otherBeforeOrEqual) {
            return Comparison.EQUAL;
        }
        if (thisBeforeOrEqual) {
            return Comparison.BEFORE;
        }
        if (otherBeforeOrEqual) {
            return Comparison.AFTER;
        }
        return Comparison.CONCURRENT;
    }

    /**
     * Returns the logical clock value for the given instance, or 0 if the instance
     * has not yet recorded any events.
     *
     * @param instanceId the instance ID to look up
     * @return the logical clock value
     */
    public long get(String instanceId) {
        return entries.getOrDefault(instanceId, 0L);
    }

    @Override
    public String toString() {
        return "VectorClock" + entries;
    }
}
