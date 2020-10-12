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
package com.shieldblaze.expressgateway.common;

import java.time.Duration;
import java.util.Map;

/**
 * {@link SelfExpiringMap} automatically expires entries using a dedicated Cleaner.
 * When {@link SelfExpiringMap} is no longer used, {@link #shutdown()} must be called to shutdown
 * the Cleaner else it'll keep running in it's dedicated {@link Thread}.
 *
 * @param <K> Key
 * @param <V> Value
 */
public final class SelfExpiringMap<K, V> extends ExpiringMap<K, V> {

    private final Cleaner<K, V> cleaner = new Cleaner<>(this);

    public SelfExpiringMap(Duration duration) {
        super(duration);
        cleaner.start();
    }

    public SelfExpiringMap(Map<K, V> storageMap, Duration duration, boolean autoRenew) {
        super(storageMap, duration, autoRenew);
        cleaner.start();
    }

    /**
     * Shutdown the Cleaner.
     */
    public void shutdown() {
        cleaner.shutdown();
    }

    @Override
    public String toString() {
        return super.toString();
    }

    private static final class Cleaner<K, V> extends Thread {

        private final SelfExpiringMap<K, V> expiringMap;
        private boolean isShutdown;

        private Cleaner(SelfExpiringMap<K, V> expiringMap) {
            super("SelfExpiringMap-Cleaner");
            this.expiringMap = expiringMap;
        }

        @Override
        public void run() {
            while (!isShutdown) {
                for (Entry<K, V> entry : expiringMap.entrySet()) {
                    if (expiringMap.isExpired(entry.getKey())) {
                        expiringMap.remove(entry.getKey());
                    }
                }

                try {
                    Thread.sleep(100L);
                } catch (InterruptedException e) {
                    e.printStackTrace(); // This should never happen
                }
            }
        }

        private void shutdown() {
            if (!isShutdown) {
                isShutdown = true;
            }
        }
    }
}
