package com.shieldblaze.expressgateway.common.map;

import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;

import java.io.IOException;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

/**
 * Default Cleaner which uses {@link GlobalExecutors} for scheduling cleaning.
 *
 * @param <K> Key
 * @param <V> Value
 */
final class DefaultCleaner<K, V> extends Cleaner<K, V> {

    /**
     * Default cleanup interval in seconds. Increased from 1s to reduce CPU usage
     * for long-TTL maps where second-level precision is unnecessary.
     */
    private static final int DEFAULT_CLEANUP_INTERVAL_SECONDS = 2;

    private final ScheduledFuture<?> scheduledFuture;

    DefaultCleaner(SelfExpiringMap<K, V> selfExpiringMap) {
        this(selfExpiringMap, DEFAULT_CLEANUP_INTERVAL_SECONDS, TimeUnit.SECONDS);
    }

    DefaultCleaner(SelfExpiringMap<K, V> selfExpiringMap, int interval, TimeUnit unit) {
        super(selfExpiringMap);
        scheduledFuture = GlobalExecutors.submitTaskAndRunEvery(this, interval, interval, unit);
    }

    @Override
    public void close() throws IOException {
        scheduledFuture.cancel(true);
        super.close();
    }
}
