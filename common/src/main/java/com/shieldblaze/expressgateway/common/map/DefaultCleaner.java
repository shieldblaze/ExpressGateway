package com.shieldblaze.expressgateway.common.map;

import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;

import java.util.concurrent.TimeUnit;

/**
 * Default Cleaner which uses {@link GlobalExecutors} for scheduling cleaning.
 *
 * @param <K> Key
 * @param <V> Value
 */
final class DefaultCleaner<K, V> extends Cleaner<K, V> {

    DefaultCleaner(SelfExpiringMap<K, V> selfExpiringMap) {
        super(selfExpiringMap);
        GlobalExecutors.INSTANCE.submitTaskAndRunEvery(this, 1, 1, TimeUnit.SECONDS);
    }
}
