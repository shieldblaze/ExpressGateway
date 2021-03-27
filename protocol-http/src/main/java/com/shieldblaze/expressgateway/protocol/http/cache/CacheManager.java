/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.protocol.http.cache;

import com.github.benmanes.caffeine.cache.Cache;
import com.github.benmanes.caffeine.cache.Caffeine;
import com.github.benmanes.caffeine.cache.Expiry;
import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.configuration.http.cache.QueryStringCacheBehaviour;
import org.checkerframework.checker.index.qual.NonNegative;

import java.io.Closeable;
import java.io.IOException;
import java.net.MalformedURLException;
import java.net.URI;
import java.net.URL;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;

@SuppressWarnings("NullableProblems")
public final class CacheManager implements Runnable, Closeable {

    private final ScheduledFuture<?> scheduledFuture;
    private QueryStringCacheBehaviour queryStringCacheBehaviour;

    /**
     * Create a new {@link CacheManager} Instance
     */
    public CacheManager(QueryStringCacheBehaviour queryStringCacheBehaviour) {
        scheduledFuture = GlobalExecutors.INSTANCE.submitTaskAndRunEvery(this, 0, 1, TimeUnit.SECONDS);
        queryStringCacheBehaviour(queryStringCacheBehaviour);
    }

    public void put(String key, Cached cached) throws MalformedURLException {
        switch (queryStringCacheBehaviour) {
            case STANDARD:
                CAFFEINE_CACHE.put(buildURL(key, true), cached);
            case IGNORE_QUERY_STRING:
                CAFFEINE_CACHE.put(buildURL(key, false), cached);
            case NO_QUERY_STRING:
            default:
                // Nothing will happen
        }
    }

    public Cached lookup(String key) throws MalformedURLException {
        switch (queryStringCacheBehaviour) {
            case STANDARD:
                return CAFFEINE_CACHE.getIfPresent(buildURL(key, true));
            case IGNORE_QUERY_STRING:
               return CAFFEINE_CACHE.getIfPresent(buildURL(key, false));
            case NO_QUERY_STRING:
            default:
                return null;
        }
    }

    public void invalidate(String key) {
        CAFFEINE_CACHE.asMap().forEach((path, cached) -> {
            try {
                if (pathValidator(path, key)) {
                    close(cached);
                    CAFFEINE_CACHE.invalidate(path);
                }
            } catch (Exception ex) {
                // Ignore
            }
        });
    }

    @Override
    public void run() {
        CAFFEINE_CACHE.asMap().forEach((key, cached) -> {
            if (cached.hasExpired()) {
                close(cached);
                CAFFEINE_CACHE.invalidate(key);
            }
        });
    }

    @Override
    public void close() {
        scheduledFuture.cancel(true);
        cleanUp();
    }

    private void cleanUp() {
        CAFFEINE_CACHE.asMap().forEach((key, cached) -> close(cached));
        CAFFEINE_CACHE.invalidateAll();
    }

    private void close(Cached cached) {
        if (cached.isByteBuf()) {
            cached.byteBuf().release();
        } else {
            try {
                cached.randomAccessFile().close();
            } catch (IOException e) {
                // Ignore
            }
        }
    }

    public QueryStringCacheBehaviour queryStringCacheBehaviour() {
        return queryStringCacheBehaviour;
    }

    @NonNull
    public void queryStringCacheBehaviour(QueryStringCacheBehaviour queryStringCacheBehaviour) {
        this.queryStringCacheBehaviour = queryStringCacheBehaviour;
        cleanUp();
    }

    public static String buildURL(String urlAsString, boolean includeQuery) throws MalformedURLException {
        URL url = new URL(urlAsString);

        StringBuilder urlBuilder = new StringBuilder();
        urlBuilder.append(url.getProtocol()).append("://").append(url.getHost()); // Protocol://Host

        if (url.getPort() != -1) {
            urlBuilder.append(":").append(url.getPort()); // :Port
        }

        urlBuilder.append(url.getPath()); // /Path

        if (includeQuery && url.getQuery() != null) {
            urlBuilder.append("?").append(url.getQuery()); // ?Query
        }

        return urlBuilder.toString();
    }

    public static boolean pathValidator(String OriginalPath, String ValidationPath) throws MalformedURLException {

        URL originalPath = new URL(OriginalPath);
        URL validationPath = new URL(ValidationPath);

        if (!validationPath.getHost().equalsIgnoreCase(originalPath.getHost())) {
            return false;
        }

        if (validationPath.getPort() != -1) {
            if (validationPath.getPort() != originalPath.getPort()) {
                return false;
            }
        }

        if (!validationPath.getProtocol().equalsIgnoreCase(originalPath.getProtocol())) {
            return false;
        }

        boolean wasSuccess = false;
        int iterationCount = 0;

        try {

            // Iterate over Original Path
            for (String path : originalPath.getPath().split("/")) {

                /*
                 * 1. If Original Path has '$ALL' Flag and Validation Path has more Paths then we'll break
                 * iteration and mark success because '$ALL' include all next path blocks.
                 *
                 * 2. If Original Path has '$DMC' Flag then we'll continue to next iteration.
                 *
                 * 3. We'll match Original Path with Validation Path.
                 */
                if ("$ALL".equals(validationPath.getPath().split("/")[iterationCount]) &&
                        (validationPath.getPath().split("/")[iterationCount] != null)) {
                    wasSuccess = true;
                    break;
                } else if ("$DMC".equals(validationPath.getPath().split("/")[iterationCount]) ||
                        path.equals(validationPath.getPath().split("/")[iterationCount])) {
                    wasSuccess = true;
                    iterationCount++;
                } else {
                    wasSuccess = false;
                    break;
                }
            }

            return wasSuccess;
        } catch (ArrayIndexOutOfBoundsException ex) {
            return false;
        }
    }

    /**
     * String: FQDN with complete Path
     * Cached: Cached data
     */
    private final Cache<String, Cached> CAFFEINE_CACHE = Caffeine.newBuilder()
            .expireAfter(new Expiry<String, Cached>() {
                @Override
                public long expireAfterCreate(String s, Cached cached, long l) {
                    return Long.MAX_VALUE; // We don't want automatic expiration
                }

                @Override
                public long expireAfterUpdate(String s, Cached cached, long l, @NonNegative long l1) {
                    return Long.MAX_VALUE; // We don't want automatic expiration
                }

                @Override
                public long expireAfterRead(String s, Cached cached, long l, @NonNegative long l1) {
                    return Long.MAX_VALUE; // We don't want automatic expiration
                }
            })
            .build();
}
