/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Collections;
import java.util.HexFormat;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CopyOnWriteArrayList;

public final class StickySession implements SessionPersistence<HTTPBalanceResponse, HTTPBalanceResponse, HTTPBalanceRequest, Node> {

    private static final String COOKIE_NAME = "X-SBZ-EGW-RouteID";

    private final List<Node> nodes = new CopyOnWriteArrayList<>();

    /**
     * HIGH-07: Pre-computed map of SHA-256(nodeId) -> Node. Eliminates O(n * SHA-256)
     * per-request lookup in findNodeByHashedId(). Updated in addIfAbsent() and remove().
     */
    private final Map<String, Node> hashedIdToNode = new ConcurrentHashMap<>();

    /**
     * LB-07: When true, the Set-Cookie header will include the Secure flag.
     * RFC 6265 Section 4.1.2.5: A cookie with the Secure attribute is only
     * sent to the server over TLS, preventing session ID leakage over
     * plaintext connections. This should be set to true when the frontend
     * listener is configured with TLS termination.
     */
    private volatile boolean secure;

    /**
     * Set whether cookies should include the Secure flag (for HTTPS listeners).
     *
     * @param secure {@code true} to add the Secure flag to sticky session cookies
     * @return this instance for fluent configuration
     */
    public StickySession setSecure(boolean secure) {
        this.secure = secure;
        return this;
    }

    /**
     * @return {@code true} if cookies will include the Secure flag
     */
    public boolean isSecure() {
        return secure;
    }

    @Override
    public HTTPBalanceResponse node(Request request) {
        return getBackend((HTTPBalanceRequest) request);
    }

    public HTTPBalanceResponse getBackend(HTTPBalanceRequest httpBalanceRequest) {
        if (httpBalanceRequest.http2Headers() != null) {
            if (httpBalanceRequest.http2Headers().contains(HttpHeaderNames.COOKIE)) {
                List<CharSequence> cookies = httpBalanceRequest.http2Headers().getAll(HttpHeaderNames.COOKIE);
                for (CharSequence cookieAsString : cookies) {
                    Cookie cookie = ClientCookieDecoder.STRICT.decode(cookieAsString.toString());
                    if (cookie.name().equalsIgnoreCase(COOKIE_NAME)) {
                        try {
                            // MED-27: Cookie now contains hashed node ID
                            String hashedValue = cookie.value();
                            Node matched = findNodeByHashedId(hashedValue);
                            if (matched != null) {
                                return new HTTPBalanceResponse(matched, EmptyHttpHeaders.INSTANCE);
                            }
                        } catch (Exception ex) {
                            break;
                        }
                    }
                }
            }
        } else if (httpBalanceRequest.httpHeaders() != null) {
            if (httpBalanceRequest.httpHeaders().contains(HttpHeaderNames.COOKIE)) {
                List<String> cookies = httpBalanceRequest.httpHeaders().getAllAsString(HttpHeaderNames.COOKIE);
                for (String cookieAsString : cookies) {
                    Cookie cookie = ClientCookieDecoder.STRICT.decode(cookieAsString);
                    if (cookie.name().equalsIgnoreCase(COOKIE_NAME)) {
                        try {
                            // MED-27: Cookie now contains hashed node ID
                            String hashedValue = cookie.value();
                            Node matched = findNodeByHashedId(hashedValue);
                            if (matched != null) {
                                return new HTTPBalanceResponse(matched, EmptyHttpHeaders.INSTANCE);
                            }
                        } catch (Exception ex) {
                            break;
                        }
                    }
                }
            }
        }

        return null;
    }

    @Override
    public HTTPBalanceResponse addRoute(HTTPBalanceRequest httpBalanceRequest, Node node) {
        // MED-27: Hash the node ID to avoid leaking internal UUIDs
        DefaultCookie cookie = new DefaultCookie(COOKIE_NAME, hashNodeId(node.id()));

        // LB-08: Extract host from either HTTP/1.1 Host header or HTTP/2 :authority
        // pseudo-header. HTTP/2 does not use the Host header; the :authority pseudo-header
        // serves the same purpose (RFC 9113 Section 8.3.1).
        String host = null;
        if (httpBalanceRequest.httpHeaders() != null) {
            host = httpBalanceRequest.httpHeaders().get(HttpHeaderNames.HOST);
        } else if (httpBalanceRequest.http2Headers() != null) {
            CharSequence authority = httpBalanceRequest.http2Headers().authority();
            if (authority != null) {
                host = authority.toString();
            }
        }

        if (host != null) {
            // LB-06: Strip port from Host header before setting cookie domain.
            // RFC 6265 Section 4.1.2.3: the Domain attribute must not include a port.
            // Handle both IPv4 ("example.com:8080") and IPv6 ("[::1]:8080") formats.
            String domain;
            if (host.startsWith("[")) {
                // IPv6 literal: "[::1]:8080" or "[::1]"
                int bracket = host.indexOf(']');
                domain = bracket > 0 ? host.substring(0, bracket + 1) : host;
            } else {
                int lastColon = host.lastIndexOf(':');
                domain = lastColon > 0 ? host.substring(0, lastColon) : host;
            }
            cookie.setDomain(domain);
        }
        cookie.setPath("/");
        cookie.setHttpOnly(true);
        cookie.setSameSite(CookieHeaderNames.SameSite.Strict);

        // LB-07: Set the Secure flag when the frontend connection is over TLS.
        // RFC 6265 Section 4.1.2.5: prevents the cookie from being sent over
        // plaintext HTTP, protecting the session ID from eavesdropping.
        if (secure) {
            cookie.setSecure(true);
        }

        DefaultHttpHeaders defaultHttpHeaders = new DefaultHttpHeaders();
        defaultHttpHeaders.add(HttpHeaderNames.SET_COOKIE, ServerCookieEncoder.STRICT.encode(cookie));

        addIfAbsent(node);

        return new HTTPBalanceResponse(node, defaultHttpHeaders);
    }

    @Override
    public boolean removeRoute(HTTPBalanceRequest httpBalanceRequest, Node node) {
        hashedIdToNode.remove(hashNodeId(node.id()));
        return nodes.remove(node);
    }

    @Override
    public boolean remove(Node node) {
        hashedIdToNode.remove(hashNodeId(node.id()));
        return nodes.remove(node);
    }

    @Override
    public void clear() {
        nodes.clear();
        hashedIdToNode.clear();
    }

    @Override
    public String toString() {
        return "StickySession{" +
                "nodes=" + nodes +
                '}';
    }

    @Override
    public String name() {
        return "StickySession";
    }

    /**
     * MED-27: Find a node whose hashed ID matches the cookie value.
     * HIGH-07: O(1) lookup via pre-computed hashedIdToNode map instead of
     * O(n * SHA-256) linear scan on every request.
     */
    private Node findNodeByHashedId(String hashedValue) {
        return hashedIdToNode.get(hashedValue);
    }

    /**
     * MED-27: Hash a node ID with SHA-256 to avoid leaking internal UUIDs in cookies.
     */
    private static String hashNodeId(String nodeId) {
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            byte[] hash = digest.digest(nodeId.getBytes(StandardCharsets.UTF_8));
            return HexFormat.of().formatHex(hash);
        } catch (NoSuchAlgorithmException e) {
            throw new AssertionError("SHA-256 not available", e);
        }
    }

    private void addIfAbsent(Node node) {
        synchronized (nodes) {
            if (!nodes.contains(node)) {
                nodes.add(node);
                Collections.sort(nodes);
                // HIGH-07: Pre-compute and cache the SHA-256 hash for O(1) lookup.
                hashedIdToNode.put(hashNodeId(node.id()), node);
            }
        }
    }
}
