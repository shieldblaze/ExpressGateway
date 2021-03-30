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

import com.shieldblaze.expressgateway.configuration.http.cache.QueryStringCacheBehaviour;
import io.netty.buffer.Unpooled;
import org.junit.jupiter.api.Test;

import java.net.MalformedURLException;
import java.nio.charset.StandardCharsets;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;

class CacheManagerTest {

    @Test
    void testStandardCache() throws MalformedURLException {
        try (CacheManager cacheManager = new CacheManager(QueryStringCacheBehaviour.STANDARD)) {

            cacheManager.put("https://shieldblaze.com/index.html", new Cached(Unpooled.wrappedBuffer("Meow".getBytes()), 60));
            assertNotNull(cacheManager.lookup("https://shieldblaze.com/index.html"));
            assertEquals("Meow", cacheManager.lookup("https://shieldblaze.com/index.html").byteBuf().toString(StandardCharsets.UTF_8));
            assertNull(cacheManager.lookup("https://shieldblaze.com/index.html?id=1"));
        }
    }

    @Test
    void testIgnoreQueryString() throws MalformedURLException {
        try (CacheManager cacheManager = new CacheManager(QueryStringCacheBehaviour.IGNORE_QUERY_STRING)) {
            cacheManager.put("https://shieldblaze.com/index.html", new Cached(Unpooled.wrappedBuffer("Meow".getBytes()), 60));
            assertNotNull(cacheManager.lookup("https://shieldblaze.com/index.html?query=hey"));
            assertEquals("Meow", cacheManager.lookup("https://shieldblaze.com/index.html?cat=meow").byteBuf().toString(StandardCharsets.UTF_8));
        }
    }

    @Test
    void testStandardInvalidateTest() throws MalformedURLException {
        try (CacheManager cacheManager = new CacheManager(QueryStringCacheBehaviour.STANDARD)) {
            for (int i = 0; i < 100; i++) {
                cacheManager.put("https://shieldblaze.com/" + i, new Cached(Unpooled.wrappedBuffer("Meow".getBytes()), 60));
                assertEquals("Meow", cacheManager.lookup("https://shieldblaze.com/" + i).byteBuf().toString(StandardCharsets.UTF_8));
            }

            cacheManager.invalidate("https://shieldblaze.com/$ALL");

            for (int i = 0; i < 100; i++) {
                assertNull(cacheManager.lookup("https://shieldblaze.com/" + i));
            }
        }
    }

    @Test
    void testAutoExpire() throws MalformedURLException {
        try (CacheManager cacheManager = new CacheManager(QueryStringCacheBehaviour.STANDARD)) {
            cacheManager.put("https://shieldblaze.com/main", new Cached(Unpooled.wrappedBuffer("Meow".getBytes()), 10));
            assertEquals("Meow", cacheManager.lookup("https://shieldblaze.com/main").byteBuf().toString(StandardCharsets.UTF_8));

            Thread.sleep(1000 * 15);

            assertNull(cacheManager.lookup("https://shieldblaze.com/main"));
        } catch (InterruptedException e) {
            // Ignore
        }
    }

    @Test
    void testURLBuilderStandard() throws MalformedURLException {
        assertEquals("https://shieldblaze.com/meow", CacheManager.buildURL("https://shieldblaze.com/meow", true));
        assertEquals("https://shieldblaze.com/meow?id=1", CacheManager.buildURL("https://shieldblaze.com/meow?id=1", true));
        assertEquals("https://shieldblaze.com/meow", CacheManager.buildURL("https://shieldblaze.com/meow?id=1", false));
    }
}
