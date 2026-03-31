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
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;

/**
 * Translates HTTP/1.1 messages to HTTP/2 frames.
 *
 * <h3>Request translation (RFC 9113 Section 8.3.1)</h3>
 * <ul>
 *   <li>HTTP/1.1 request line → :method, :path pseudo-headers</li>
 *   <li>Host header → :authority pseudo-header</li>
 *   <li>TLS context → :scheme pseudo-header</li>
 *   <li>Hop-by-hop headers (Connection, Keep-Alive, TE, Transfer-Encoding, Upgrade) → stripped</li>
 *   <li>Cookie header → split per RFC 9113 Section 8.2.3</li>
 *   <li>Request body (chunked or content-length) → DATA frames</li>
 *   <li>LastHttpContent → DATA frame with endStream=true, or HEADERS frame for trailers</li>
 * </ul>
 *
 * <h3>Response translation (reverse path)</h3>
 * <ul>
 *   <li>HTTP/1.1 status line → :status pseudo-header in HEADERS frame</li>
 *   <li>Response body → DATA frames</li>
 *   <li>Trailing headers → trailing HEADERS frame with endStream=true</li>
 * </ul>
 */
public final class H1ToH2Translator implements ProtocolTranslator {

    static final H1ToH2Translator INSTANCE = new H1ToH2Translator();

    private H1ToH2Translator() {
    }

    @Override
    public ProtocolVersion sourceProtocol() {
        return ProtocolVersion.HTTP_1_1;
    }

    @Override
    public ProtocolVersion targetProtocol() {
        return ProtocolVersion.HTTP_2;
    }

    /**
     * Translates an HTTP/1.1 request to HTTP/2 HEADERS frame.
     *
     * <p>If the request is a FullHttpRequest with no body, the returned
     * HEADERS frame has endStream=true. Otherwise, endStream=false and
     * the caller must send DATA frames for the body and a final frame
     * with endStream=true.</p>
     *
     * @param request the HTTP/1.1 request
     * @param isTls   whether the frontend connection uses TLS
     * @return the HTTP/2 HEADERS frame
     */
    public Http2HeadersFrame translateRequest(HttpRequest request, boolean isTls) {
        Http2Headers h2Headers = HeaderTransformer.h1RequestToH2(request, isTls);
        HeaderTransformer.addVia(h2Headers, ProtocolVersion.HTTP_1_1);

        boolean endStream = request instanceof FullHttpRequest fullReq
                && fullReq.content().readableBytes() == 0;

        return new DefaultHttp2HeadersFrame(h2Headers, endStream);
    }

    /**
     * Translates an HTTP/1.1 response to HTTP/2 HEADERS frame.
     *
     * @param response the HTTP/1.1 response
     * @return the HTTP/2 HEADERS frame
     */
    public Http2HeadersFrame translateResponse(HttpResponse response) {
        Http2Headers h2Headers = HeaderTransformer.h1ResponseToH2(response);
        HeaderTransformer.addVia(h2Headers, ProtocolVersion.HTTP_1_1);

        boolean endStream = response instanceof FullHttpResponse fullResp
                && fullResp.content().readableBytes() == 0;

        return new DefaultHttp2HeadersFrame(h2Headers, endStream);
    }

    /**
     * Translates an HTTP/1.1 body chunk to an HTTP/2 DATA frame.
     *
     * <p>The caller is responsible for retaining the content's ByteBuf
     * if the source message is released after this call.</p>
     *
     * @param content     the HTTP/1.1 body chunk
     * @param endStream   whether this is the last DATA frame
     * @return the HTTP/2 DATA frame
     */
    public Http2DataFrame translateBody(HttpContent content, boolean endStream) {
        ByteBuf data = content.content().retain();
        return new DefaultHttp2DataFrame(data, endStream);
    }

    /**
     * Translates HTTP/1.1 trailing headers to an HTTP/2 trailing HEADERS frame.
     *
     * <p>Per RFC 9113 Section 8.1, trailers are sent as a HEADERS frame with
     * endStream=true after all DATA frames.</p>
     *
     * @param lastContent the LastHttpContent containing trailing headers
     * @return the HTTP/2 trailing HEADERS frame with endStream=true
     */
    public Http2HeadersFrame translateTrailers(LastHttpContent lastContent) {
        Http2Headers trailers = HeaderTransformer.h1TrailersToH2(lastContent.trailingHeaders());
        return new DefaultHttp2HeadersFrame(trailers, true);
    }
}
