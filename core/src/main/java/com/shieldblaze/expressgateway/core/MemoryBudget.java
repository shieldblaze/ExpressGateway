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
package com.shieldblaze.expressgateway.core;

import java.lang.management.ManagementFactory;
import java.lang.management.RuntimeMXBean;
import java.util.concurrent.atomic.AtomicLong;

public final class MemoryBudget {

    private static final long DEFAULT_MAX_DIRECT_MEMORY_BYTES = 1L * 1024 * 1024 * 1024;
    private static final double DEFAULT_BUDGET_RATIO = 0.75;

    private final long maxDirectMemoryBytes;
    private final AtomicLong usageCounter;

    public MemoryBudget(long maxDirectMemoryBytes) {
        if (maxDirectMemoryBytes <= 0) {
            throw new IllegalArgumentException("maxDirectMemoryBytes must be positive: " + maxDirectMemoryBytes);
        }
        this.maxDirectMemoryBytes = maxDirectMemoryBytes;
        this.usageCounter = new AtomicLong(0L);
    }

    public MemoryBudget() {
        this(resolveDefaultMaxBytes());
    }

    public boolean tryAcquire(long bytes) {
        if (bytes < 0) {
            throw new IllegalArgumentException("bytes must be non-negative: " + bytes);
        }
        if (bytes == 0) {
            return true;
        }

        long current;
        long newValue;
        do {
            current = usageCounter.get();
            newValue = current + bytes;
            if (newValue > maxDirectMemoryBytes || newValue < 0) {
                return false;
            }
        } while (!usageCounter.compareAndSet(current, newValue));

        return true;
    }

    public void release(long bytes) {
        if (bytes < 0) {
            throw new IllegalArgumentException("bytes must be non-negative: " + bytes);
        }
        if (bytes == 0) {
            return;
        }

        long current;
        do {
            current = usageCounter.get();
            if (current < bytes) {
                throw new IllegalStateException(
                        "Released more memory than acquired: attempted to release " + bytes +
                        " bytes but only " + current + " bytes were used");
            }
        } while (!usageCounter.compareAndSet(current, current - bytes));
    }

    public long getUsedBytes() {
        return usageCounter.get();
    }

    public long getMaxBytes() {
        return maxDirectMemoryBytes;
    }

    public double getUsageRatio() {
        return (double) usageCounter.get() / maxDirectMemoryBytes;
    }

    public boolean isOverThreshold(double threshold) {
        if (threshold < 0.0 || threshold > 1.0) {
            throw new IllegalArgumentException("threshold must be in [0.0, 1.0]: " + threshold);
        }
        return usageCounter.get() > (long) (maxDirectMemoryBytes * threshold);
    }

    private static long resolveDefaultMaxBytes() {
        try {
            RuntimeMXBean runtimeMxBean = ManagementFactory.getRuntimeMXBean();
            for (String arg : runtimeMxBean.getInputArguments()) {
                if (arg.startsWith("-XX:MaxDirectMemorySize=")) {
                    String value = arg.substring("-XX:MaxDirectMemorySize=".length());
                    long parsed = parseMemorySize(value);
                    if (parsed > 0) {
                        return (long) (parsed * DEFAULT_BUDGET_RATIO);
                    }
                }
            }
        } catch (Exception _) {
            // SecurityManager or other restriction -- fall through to default
        }
        return DEFAULT_MAX_DIRECT_MEMORY_BYTES;
    }

    static long parseMemorySize(String value) {
        if (value == null || value.isEmpty()) {
            return -1;
        }
        value = value.trim();
        char lastChar = value.charAt(value.length() - 1);
        long multiplier = 1;
        if (Character.isLetter(lastChar)) {
            multiplier = switch (Character.toLowerCase(lastChar)) {
                case 'k' -> 1024L;
                case 'm' -> 1024L * 1024;
                case 'g' -> 1024L * 1024 * 1024;
                default -> -1L;
            };
            if (multiplier == -1L) {
                return -1;
            }
            value = value.substring(0, value.length() - 1);
        }
        try {
            return Long.parseLong(value) * multiplier;
        } catch (NumberFormatException _) {
            return -1;
        }
    }

    @Override
    public String toString() {
        return "MemoryBudget{" +
                "used=" + getUsedBytes() +
                ", max=" + maxDirectMemoryBytes +
                ", ratio=" + String.format("%.2f", getUsageRatio()) +
                '}';
    }
}
