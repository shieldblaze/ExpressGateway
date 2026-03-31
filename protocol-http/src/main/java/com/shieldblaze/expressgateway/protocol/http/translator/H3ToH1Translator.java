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
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultHttpRequest;
import io.netty.handler.codec.http.DefaultHttpResponse;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaders;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http.LastHttpContent;
import io.netty.handler.codec.http3.Http3DataFrame;
import io.netty.handler.codec.http3.Http3Headers;
import io.netty.handler.codec.http3.Http3HeadersFrame;

/**
 * Translates HTTP/3 frames to HTTP/1.1 messages.
 *
 * <h3>Request translation (RFC 9114 Section 4.3 → RFC 9112)</h3>
 * <ul>
 *   <li>:method, :path → HTTP/1.1 request line</li>
 *   <li>:authority → Host header</li>
 *   <li>:scheme → dropped (implicit in H1 connection context)</li>
 *   <li>DATA frames → body chunks</li>
 *   <li>QUIC stream FIN → LastHttpContent (end of request)</li>
 *   <li>Trailing HEADERS frame → HTTP/1.1 chunked trailers</li>
 * </ul>
 *
 * <h3>QUIC stream → TCP connection mapping</h3>
 * Each HTTP/3 request occupies one bidirectional QUIC stream. In HTTP/1.1,
 * each request occupies the TCP connection serially. The translation layer
 * maps each QUIC stream to an independent backend TCP connection (via the
 * connection pool). Stream independence is preserved: loss/retransmission on
 * one QUIC stream does not affect other streams, while TCP connections are
 * affected by head-of-line blocking.
 *
 * <h3>Connection migration</h3>
 * HTTP/3 supports connection migration (RFC 9000 Section 9) where the client
 * changes IP address/port. This is transparent to the proxy: the QUIC layer
 * handles migration, and the HTTP/3 streams remain associated with their
 * backend TCP connections regardless of client migration events.
 */
public final class H3ToH1Translator implements ProtocolTranslator {

    static final H3ToH1Translator INSTANCE = new H3ToH1Translator();

    private H3ToH1Translator() {
    }

    @Override
    public ProtocolVersion sourceProtocol() {
        return ProtocolVersion.HTTP_3;
    }

    @Override
    public ProtocolVersion targetProtocol() {
        return ProtocolVersion.HTTP_1_1;
    }

    /**
     * Translates an HTTP/3 request HEADERS frame to an HTTP/1.1 request.
     *
     * @param headersFrame the HTTP/3 request HEADERS frame
     * @return the HTTP/1.1 request
     * @throws IllegalArgumentException if required pseudo-headers are missing
     */
    public HttpRequest translateRequest(Http3HeadersFrame headersFrame) {
        Http3Headers h3 = headersFrame.headers();

        HttpMethod method = HeaderTransformer.h3RequestMethod(h3);
        String path = HeaderTransformer.h3RequestPath(h3);
        HttpHeaders h1Headers = HeaderTransformer.h3RequestToH1Headers(h3);

        DefaultHttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, method, path, h1Headers);
        HeaderTransformer.addVia(request.headers(), ProtocolVersion.HTTP_3);
        return request;
    }

    /**
     * Translates an HTTP/3 response HEADERS frame to an HTTP/1.1 response.
     *
     * @param headersFrame the HTTP/3 response HEADERS frame
     * @return the HTTP/1.1 response
     */
    public HttpResponse translateResponse(Http3HeadersFrame headersFrame) {
        Http3Headers h3 = headersFrame.headers();

        int statusCode = HeaderTransformer.h3ResponseStatus(h3);
        HttpHeaders h1Headers = HeaderTransformer.h3ResponseToH1Headers(h3);

        DefaultHttpResponse response = new DefaultHttpResponse(
                HttpVersion.HTTP_1_1, HttpResponseStatus.valueOf(statusCode), h1Headers);
        HeaderTransformer.addVia(response.headers(), ProtocolVersion.HTTP_3);
        return response;
    }

    /**
     * Translates an HTTP/3 DATA frame to an HTTP/1.1 body chunk.
     *
     * @param dataFrame the HTTP/3 DATA frame
     * @return HTTP/1.1 body chunk
     */
    public HttpContent translateBody(Http3DataFrame dataFrame) {
        ByteBuf content = dataFrame.content().retain();
        return new DefaultHttpContent(content);
    }

    /**
     * Translates HTTP/3 trailing HEADERS frame to HTTP/1.1 LastHttpContent with trailers.
     *
     * @param headersFrame the HTTP/3 trailing HEADERS frame
     * @return LastHttpContent with trailing headers
     */
    public LastHttpContent translateTrailers(Http3HeadersFrame headersFrame) {
        HttpHeaders h1Trailers = HeaderTransformer.h3TrailersToH1(headersFrame.headers());
        DefaultLastHttpContent lastContent = new DefaultLastHttpContent();
        lastContent.trailingHeaders().add(h1Trailers);
        return lastContent;
    }

    /**
     * Creates an empty LastHttpContent to signal end-of-message when the QUIC
     * stream is closed (FIN received) without trailing headers.
     *
     * @return empty LastHttpContent
     */
    public LastHttpContent endOfMessage() {
        return LastHttpContent.EMPTY_LAST_CONTENT;
    }
}
