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

    private final ScheduledFuture<?> scheduledFuture;

    DefaultCleaner(SelfExpiringMap<K, V> selfExpiringMap) {
        super(selfExpiringMap);
        scheduledFuture = GlobalExecutors.INSTANCE.submitTaskAndRunEvery(this, 1, 1, TimeUnit.SECONDS);
    }

    @Override
    public void close() throws IOException {
        scheduledFuture.cancel(true);
        super.close();
    }
}
