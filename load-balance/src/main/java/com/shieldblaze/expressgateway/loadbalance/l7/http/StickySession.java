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
package com.shieldblaze.expressgateway.loadbalance.l7.http;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.common.list.RoundRobinList;
import com.shieldblaze.expressgateway.loadbalance.SessionPersistence;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.EmptyHttpHeaders;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.cookie.ClientCookieDecoder;
import io.netty.handler.codec.http.cookie.Cookie;
import io.netty.handler.codec.http.cookie.CookieHeaderNames;
import io.netty.handler.codec.http.cookie.DefaultCookie;
import io.netty.handler.codec.http.cookie.ServerCookieEncoder;

import java.util.List;
import java.util.Optional;

public final class StickySession extends HTTPL7Balance {

    private RoundRobinList<Backend> backendsRoundRobin;

    public StickySession(SessionPersistence<Backend, HTTPRequest, HTTPResponse> sessionPersistence) {
        super(sessionPersistence);
    }

    @Override
    public void setBackends(List<Backend> backends) {
        super.setBackends(backends);
        backendsRoundRobin = new RoundRobinList<>(this.backends);
    }

    @Override
    public HTTPResponse getBackend(HTTPRequest HTTPRequest) {
        Backend backend = sessionPersistence.getBackend(HTTPRequest);
        if (backend != null) {
            return new HTTPResponse(backend, EmptyHttpHeaders.INSTANCE);
        }

        if (HTTPRequest.getHTTPHeaders().contains(HttpHeaderNames.COOKIE)) {
            List<String> cookies = HTTPRequest.getHTTPHeaders().getAllAsString(HttpHeaderNames.COOKIE);
            for (String _cookie : cookies) {
                Cookie cookie = ClientCookieDecoder.STRICT.decode(_cookie);
                if (cookie.name().equalsIgnoreCase("X-Route-ID")) {
                    try {
                        String value = cookie.value();

                        Optional<Backend> optionalBackend = backends.stream()
                                .filter(_backend -> _backend.getHash().equalsIgnoreCase(value))
                                .findAny();

                        if (optionalBackend.isPresent()) {
                            return new HTTPResponse(optionalBackend.get(), EmptyHttpHeaders.INSTANCE);
                        }
                    } catch (Exception ex) {
                        break;
                    }
                }
            }
        }

        backend = backendsRoundRobin.iterator().next();

        DefaultCookie cookie = new DefaultCookie("X-SBZ-EGW-RouteID", String.valueOf(backend.getHash()));
        cookie.setDomain(backend.getHostname());
        cookie.setPath("/");
        cookie.setHttpOnly(true);
        cookie.setSameSite(CookieHeaderNames.SameSite.Strict);

        DefaultHttpHeaders defaultHttpHeaders = new DefaultHttpHeaders();
        defaultHttpHeaders.add(HttpHeaderNames.SET_COOKIE, ServerCookieEncoder.STRICT.encode(cookie));

        return new HTTPResponse(backend, defaultHttpHeaders);
    }
}
