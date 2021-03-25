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
package com.shieldblaze.expressgateway.configuration.http.cache;

/**
 * Cache Behaviour in case of Query String for a given URL.
 */
public enum QueryStringCacheBehaviour {

    /**
     * <p>
     * In this mode, Query String is ignored on first request and request is sent to
     * backend. When backend responds with response, the response is cached (depends on multiple factors),
     * and returned back to client. All subsequent requests are served from Cache until the Cached
     * Response expires or is elevated.
     * </p>
     *
     * <p>
     * Examples:
     * <ul>
     *     <li> "https://www.shieldblaze.com/?meow=cat" will be cached as "https://www.shieldblaze.com/" </li>
     *     <li> "https://www.shieldblaze.com/" will be cached as "https://www.shieldblaze.com/" </li>
     *     <li> "https://www.shieldblaze.com/index.html" will be cached as "https://www.shieldblaze.com/index.html" </li>
     *     <li> "https://www.shieldblaze.com/index.html?hello=1" will be cached as "https://www.shieldblaze.com/index.html" </li>
     * </ul>
     * </p>
     */
    IGNORE_QUERY_STRING,

    /**
     * <p>
     * In this mode, all requests containing Query String are sent directly to the Backend.
     * No caching happens at all.
     * </p>
     *
     * <p>
     * Examples:
     *     <ul>
     *         <li> "https://www.shieldblaze.com/?meow=cat" will be forwarded to Backend as "https://www.shieldblaze.com/?meow=cat" </li>
     *         <li> "https://www.shieldblaze.com/" will be forwarded to Backend as "https://www.shieldblaze.com/" </li>
     *         <li> "https://www.shieldblaze.com/index.html" will be forwarded to Backend as "https://www.shieldblaze.com/index.html" </li>
     *         <li> "https://www.shieldblaze.com/index.html?hello=1" will be forwarded to Backend as "https://www.shieldblaze.com/index.html?hello=1" </li>
     *     </ul>
     * </p>
     */
    NO_QUERY_STRING,

    /**
     * <p>
     * In this mode, all requests containing Query String or not, will be cached.
     * </p>
     *
     * <p>
     *     Examples:
     *     <ul>
     *          <li> "https://www.shieldblaze.com/?meow=cat" will be cached as "https://www.shieldblaze.com/?meow=cat" </li>
     *          <li> "https://www.shieldblaze.com/" will be cached as "https://www.shieldblaze.com/" </li>
     *          <li> "https://www.shieldblaze.com/index.html" will be cached as "https://www.shieldblaze.com/index.html" </li>
     *          <li> "https://www.shieldblaze.com/index.html?hello=1" will be cached as "https://www.shieldblaze.com/index.html?hello=1" </li>
     *     </ul>
     * </p>
     */
    STANDARD
}
