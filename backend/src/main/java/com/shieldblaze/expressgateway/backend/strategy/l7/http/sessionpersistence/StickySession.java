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
package com.shieldblaze.expressgateway.backend.strategy.l7.http.sessionpersistence;

import com.shieldblaze.expressgateway.backend.Node;
import com.shieldblaze.expressgateway.backend.loadbalance.Request;
import com.shieldblaze.expressgateway.backend.loadbalance.SessionPersistence;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceRequest;
import com.shieldblaze.expressgateway.backend.strategy.l7.http.HTTPBalanceResponse;
import io.netty.handler.codec.http.DefaultHttpHeaders;
import io.netty.handler.codec.http.EmptyHttpHeaders;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.cookie.ClientCookieDecoder;
import io.netty.handler.codec.http.cookie.Cookie;
import io.netty.handler.codec.http.cookie.CookieHeaderNames;
import io.netty.handler.codec.http.cookie.DefaultCookie;
import io.netty.handler.codec.http.cookie.ServerCookieEncoder;

import java.util.Collections;
import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

public final class StickySession implements SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> {

    private static final String COOKIE_NAME = "X-SBZ-EGW-RouteID";

    private final List<Node> nodes = new CopyOnWriteArrayList<>();

    @Override
    public HTTPBalanceResponse node(Request request) {
        return getBackend((HTTPBalanceRequest) request);
    }

    public HTTPBalanceResponse getBackend(HTTPBalanceRequest httpBalanceRequest) {
        if (httpBalanceRequest.httpHeaders().contains(HttpHeaderNames.COOKIE)) {
            List<String> cookies = httpBalanceRequest.httpHeaders().getAllAsString(HttpHeaderNames.COOKIE);
            for (String cookieAsString : cookies) {
                Cookie cookie = ClientCookieDecoder.STRICT.decode(cookieAsString);
                if (cookie.name().equalsIgnoreCase(COOKIE_NAME)) {
                    try {
                        String value = cookie.value();
                        int index = Collections.binarySearch(nodes, value, StickySessionSearchComparator.INSTANCE);
                        return new HTTPBalanceResponse(nodes.get(index), EmptyHttpHeaders.INSTANCE);
                    } catch (Exception ex) {
                        break;
                    }
                }
            }
        }

        return null;
    }

    @Override
    public HTTPBalanceResponse addRoute(HTTPBalanceRequest httpBalanceRequest, Node node) {
        DefaultCookie cookie = new DefaultCookie(COOKIE_NAME, String.valueOf(node.hash()));
        cookie.setDomain(httpBalanceRequest.httpHeaders().get(HttpHeaderNames.HOST));
        cookie.setPath("/");
        cookie.setHttpOnly(true);
        cookie.setSameSite(CookieHeaderNames.SameSite.Strict);

        DefaultHttpHeaders defaultHttpHeaders = new DefaultHttpHeaders();
        defaultHttpHeaders.add(HttpHeaderNames.SET_COOKIE, ServerCookieEncoder.STRICT.encode(cookie));

        addIfAbsent(node);

        return new HTTPBalanceResponse(node, defaultHttpHeaders);
    }

    @Override
    public boolean removeRoute(HTTPBalanceRequest httpBalanceRequest, Node node) {
        return this.nodes.remove(node);
    }

    @Override
    public boolean remove(Node node) {
        return nodes.remove(node);
    }

    @Override
    public void clear() {
        nodes.clear();
    }

    private void addIfAbsent(Node node) {
        if (!nodes.contains(node)) {
            nodes.add(node);
            Collections.sort(nodes);
        }
    }
}
