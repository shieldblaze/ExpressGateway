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

/**
 * Cache Behaviour in case of Query String for a given URL.
 */
public enum QueryStringCacheBehaviour {

    /**
     * In this mode, Query String is ignored on first request and request is sent to
     * backend. When backend responds with response, the response is cached (depends on multiple factors),
     * and returned back to client. All subsequent requests are served from Cache until the Cached
     * Response expires or is elevated.
     */
    IGNORE_QUERY_STRING,

    /**
     * In this mode, all requests containing Query String are sent directly to the Backend.
     * No caching happens at all.
     */
    FORWARD_TO_BACKEND,

    /**
     * In this mode, all requests containing Query String or not, will be cached.
     */
    CACHE_ALL
}
