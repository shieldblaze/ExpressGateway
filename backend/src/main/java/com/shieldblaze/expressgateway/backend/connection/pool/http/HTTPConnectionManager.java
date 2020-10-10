package com.shieldblaze.expressgateway.backend.connection.pool.http;

import stormpot.Completion;
import stormpot.Pool;
import stormpot.Timeout;

import java.time.Duration;

public class HTTPConnectionManager {

    private static final Timeout timeout = new Timeout(Duration.ofNanos(0L));

    private final int maxAgeMillis;
    private final int maxDataBacklog;

    private Pool<HTTPConnection> pool;

    public HTTPConnectionManager(int maxAgeMillis, int maxDataBacklog) {
        this.maxAgeMillis = maxAgeMillis;
        this.maxDataBacklog = maxDataBacklog;
    }

    public void start() {
        HTTPConnectionAllocator allocator = new HTTPConnectionAllocator(maxDataBacklog);
        pool = Pool.from(allocator)
                .setExpiration(info -> {
                    // If Connection is in use, return false.
                    if (info.getPoolable().isInUse()) {
                        return false;
                    }

                    // If Connection age has crossed Max Age then we'll disconnect and release it.
                    if (info.getAgeMillis() >= maxAgeMillis) {
                        info.getPoolable().disconnect();
                        return true;
                    }

                    return false;
                })
                .setSize(Integer.MAX_VALUE)
                .build();
    }

    public HTTPConnection pool() throws InterruptedException {
        return pool.claim(timeout);
    }

    public Completion shutdown() {
        return pool.shutdown();
    }
}
