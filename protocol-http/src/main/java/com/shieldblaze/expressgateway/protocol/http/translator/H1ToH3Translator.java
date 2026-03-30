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
package com.shieldblaze.expressgateway.protocol.http.translator;

import io.netty.buffer.ByteBuf;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http.FullHttpResponse;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http3.DefaultHttp3DataFrame;
import io.netty.handler.codec.http3.DefaultHttp3HeadersFrame;
import io.netty.handler.codec.http3.Http3DataFrame;
import io.netty.handler.codec.http3.Http3Headers;
import io.netty.handler.codec.http3.Http3HeadersFrame;

/**
 * Translates HTTP/1.1 messages to HTTP/3 frames.
 *
 * <h3>Key differences from H1 → H2</h3>
 * <ul>
 *   <li>HTTP/3 frames are sent over QUIC streams (UDP), not TCP</li>
 *   <li>No endStream flag on HTTP/3 frames -- stream closure is signaled by
 *       QUIC FIN (closing the QuicStreamChannel)</li>
 *   <li>Header encoding uses QPACK (RFC 9204) instead of HPACK, but at the
 *       logical level the pseudo-headers are identical</li>
 *   <li>No connection-level flow control from the HTTP layer -- QUIC handles this</li>
 *   <li>Each request/response is on an independent stream with no head-of-line blocking</li>
 * </ul>
 *
 * <h3>Request translation</h3>
 * Same pseudo-header mapping as H1 → H2 (RFC 9114 Section 4.3.1 mirrors RFC 9113 Section 8.3.1).
 * Connection-specific headers are stripped. Body chunks become DATA frames.
 *
 * <h3>Response translation</h3>
 * :status pseudo-header in HEADERS frame. Body as DATA frames. Trailers as
 * trailing HEADERS frame. Stream closure via QUIC FIN.
 */
public final class H1ToH3Translator implements ProtocolTranslator {

    static final H1ToH3Translator INSTANCE = new H1ToH3Translator();

    private H1ToH3Translator() {
    }

    @Override
    public ProtocolVersion sourceProtocol() {
        return ProtocolVersion.HTTP_1_1;
    }

    @Override
    public ProtocolVersion targetProtocol() {
        return ProtocolVersion.HTTP_3;
    }

    /**
     * Translates an HTTP/1.1 request to an HTTP/3 HEADERS frame.
     *
     * <p>Unlike HTTP/2, HTTP/3 does not have endStream on the HEADERS frame.
     * Stream termination is signaled by closing the QUIC stream (FIN).
     * The caller must close the stream after the last DATA frame or trailer.</p>
     *
     * @param request the HTTP/1.1 request
     * @param isTls   whether the frontend connection uses TLS
     * @return the HTTP/3 HEADERS frame
     */
    public Http3HeadersFrame translateRequest(HttpRequest request, boolean isTls) {
        Http3Headers h3Headers = HeaderTransformer.h1RequestToH3(request, isTls);
        HeaderTransformer.addVia(h3Headers, ProtocolVersion.HTTP_1_1);
        return new DefaultHttp3HeadersFrame(h3Headers);
    }

    /**
     * Translates an HTTP/1.1 response to an HTTP/3 HEADERS frame.
     *
     * @param response the HTTP/1.1 response
     * @return the HTTP/3 HEADERS frame
     */
    public Http3HeadersFrame translateResponse(HttpResponse response) {
        Http3Headers h3Headers = HeaderTransformer.h1ResponseToH3(response);
        HeaderTransformer.addVia(h3Headers, ProtocolVersion.HTTP_1_1);
        return new DefaultHttp3HeadersFrame(h3Headers);
    }

    /**
     * Translates an HTTP/1.1 body chunk to an HTTP/3 DATA frame.
     *
     * @param content the HTTP/1.1 body chunk
     * @return the HTTP/3 DATA frame
     */
    public Http3DataFrame translateBody(HttpContent content) {
        ByteBuf data = content.content().retain();
        return new DefaultHttp3DataFrame(data);
    }

    /**
     * Translates HTTP/1.1 trailing headers to an HTTP/3 trailing HEADERS frame.
     *
     * <p>Per RFC 9114 Section 4.1, trailers are sent as a HEADERS frame after all
     * DATA frames. The caller must close the QUIC stream after writing this frame.</p>
     *
     * @param lastContent the LastHttpContent containing trailing headers
     * @return the HTTP/3 trailing HEADERS frame
     */
    public Http3HeadersFrame translateTrailers(LastHttpContent lastContent) {
        Http3Headers trailers = HeaderTransformer.h1TrailersToH3(lastContent.trailingHeaders());
        return new DefaultHttp3HeadersFrame(trailers);
    }

    /**
     * Returns whether the given HTTP/1.1 request has no body, meaning the QUIC
     * stream can be half-closed immediately after the HEADERS frame.
     *
     * @param request the HTTP/1.1 request
     * @return {@code true} if the request has no body content
     */
    public boolean isEmptyBody(HttpRequest request) {
        return request instanceof FullHttpRequest fullReq
                && fullReq.content().readableBytes() == 0;
    }
}
