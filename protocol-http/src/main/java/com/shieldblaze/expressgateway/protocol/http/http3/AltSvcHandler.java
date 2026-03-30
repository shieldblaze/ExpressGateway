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
package com.shieldblaze.expressgateway.protocol.http.http3;

import io.netty.channel.ChannelHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelOutboundHandlerAdapter;
import io.netty.channel.ChannelPromise;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.util.AsciiString;

/**
 * Outbound handler that injects the {@code Alt-Svc} header into HTTP/1.1 and HTTP/2
 * responses to advertise HTTP/3 availability per RFC 7838 and RFC 9114 Section 3.
 *
 * <p>Clients receiving this header may attempt QUIC-based HTTP/3 connections on
 * subsequent requests. The header format is:
 * <pre>
 *   Alt-Svc: h3=":PORT"; ma=MAX_AGE
 * </pre>
 * where {@code PORT} is the UDP port for QUIC and {@code MAX_AGE} is the cache
 * duration in seconds.</p>
 *
 * <h3>Placement in the Pipeline</h3>
 * <p>This handler must be placed after the HTTP codec (HTTP/1.1 or HTTP/2 frame codec)
 * in the outbound direction. It intercepts {@link HttpResponse} (HTTP/1.1) and
 * {@link Http2HeadersFrame} (HTTP/2 response headers) writes.</p>
 *
 * <h3>Thread Safety</h3>
 * <p>This handler is {@link ChannelHandler.Sharable} because it is stateless -- the
 * Alt-Svc header value is pre-computed and immutable after construction.</p>
 *
 * <h3>RFC Compliance</h3>
 * <ul>
 *   <li>RFC 7838 Section 3: Alt-Svc header field syntax and semantics</li>
 *   <li>RFC 9114 Section 3: HTTP/3 discovery via Alt-Svc</li>
 *   <li>RFC 9110 Section 7.6.1: Alt-Svc is a response header, not hop-by-hop</li>
 * </ul>
 */
@ChannelHandler.Sharable
public final class AltSvcHandler extends ChannelOutboundHandlerAdapter {

    private static final AsciiString ALT_SVC_HEADER_NAME = AsciiString.cached("alt-svc");

    /**
     * Pre-computed Alt-Svc header value. Immutable after construction.
     * Using AsciiString avoids per-write String-to-byte[] conversion overhead.
     */
    private final AsciiString altSvcValue;

    /**
     * Create a new {@link AltSvcHandler}.
     *
     * @param port   the UDP port for QUIC/HTTP3 listener
     * @param maxAge the max-age in seconds for the Alt-Svc cache entry;
     *               set to 0 to indicate the origin should clear any cached
     *               Alt-Svc entry (RFC 7838 Section 3)
     * @throws IllegalArgumentException if port is not in valid range [1, 65535]
     *                                  or maxAge is negative
     */
    public AltSvcHandler(int port, long maxAge) {
        if (port < 1 || port > 65535) {
            throw new IllegalArgumentException("Port must be in range [1, 65535], got: " + port);
        }
        if (maxAge < 0) {
            throw new IllegalArgumentException("Max-age must be non-negative, got: " + maxAge);
        }

        // RFC 7838 Section 3: h3=":<port>"; ma=<seconds>
        // The port is always quoted in the authority form per the ABNF.
        this.altSvcValue = AsciiString.cached("h3=\":" + port + "\"; ma=" + maxAge);
    }

    @Override
    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) throws Exception {
        if (msg instanceof HttpResponse response) {
            // HTTP/1.1 response: add Alt-Svc header.
            // Do not overwrite if already present -- an upstream handler may have set
            // a more specific Alt-Svc value (e.g., with multiple protocols).
            if (!response.headers().contains(ALT_SVC_HEADER_NAME)) {
                response.headers().set(ALT_SVC_HEADER_NAME, altSvcValue);
            }
        } else if (msg instanceof Http2HeadersFrame headersFrame) {
            // HTTP/2 response headers: add Alt-Svc pseudo-header.
            // Only inject on response headers (status present), not trailers.
            Http2Headers headers = headersFrame.headers();
            if (headers.status() != null && !headers.contains(ALT_SVC_HEADER_NAME)) {
                headers.set(ALT_SVC_HEADER_NAME, altSvcValue);
            }
        }

        ctx.write(msg, promise);
    }
}
