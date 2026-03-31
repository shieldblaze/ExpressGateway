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
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;

/**
 * Translates HTTP/2 frames to HTTP/1.1 messages.
 *
 * <h3>Request translation (RFC 9113 Section 8.3.1 → RFC 9112)</h3>
 * <ul>
 *   <li>:method, :path → HTTP/1.1 request line</li>
 *   <li>:authority → Host header</li>
 *   <li>:scheme → dropped (implicit in H1 connection context)</li>
 *   <li>Multiple cookie entries → merged with "; " per RFC 9113 Section 8.2.3</li>
 *   <li>DATA frames → body chunks or chunked encoding</li>
 *   <li>Trailing HEADERS frame (endStream=true) → HTTP/1.1 chunked trailers</li>
 *   <li>Server push → dropped (HTTP/1.1 has no push semantics; RFC 9113 deprecates push)</li>
 *   <li>RST_STREAM → connection close (HTTP/1.1 has no stream-level error signaling)</li>
 * </ul>
 *
 * <h3>Response translation</h3>
 * <ul>
 *   <li>:status → HTTP/1.1 status line</li>
 *   <li>DATA frames → body chunks</li>
 *   <li>endStream on HEADERS or DATA → LastHttpContent</li>
 * </ul>
 *
 * <h3>Flow control mapping</h3>
 * HTTP/2 uses stream+connection-level flow control windows. When translating to
 * HTTP/1.1, this maps to TCP backpressure: the proxy applies TCP-level flow control
 * (autoRead toggle) based on whether the HTTP/2 peer has available window. This is
 * handled by the {@link FlowControlTranslator}, not directly here.
 */
public final class H2ToH1Translator implements ProtocolTranslator {

    static final H2ToH1Translator INSTANCE = new H2ToH1Translator();

    private H2ToH1Translator() {
    }

    @Override
    public ProtocolVersion sourceProtocol() {
        return ProtocolVersion.HTTP_2;
    }

    @Override
    public ProtocolVersion targetProtocol() {
        return ProtocolVersion.HTTP_1_1;
    }

    /**
     * Translates an HTTP/2 request HEADERS frame to an HTTP/1.1 request.
     *
     * <p>The returned request uses HTTP/1.1 version. If the HEADERS frame has
     * endStream=true and no body is expected, the caller should send a
     * {@link LastHttpContent#EMPTY_LAST_CONTENT} immediately after.</p>
     *
     * @param headersFrame the HTTP/2 request HEADERS frame
     * @return the HTTP/1.1 request
     * @throws IllegalArgumentException if required pseudo-headers are missing
     */
    public HttpRequest translateRequest(Http2HeadersFrame headersFrame) {
        Http2Headers h2 = headersFrame.headers();

        HttpMethod method = HeaderTransformer.h2RequestMethod(h2);
        String path = HeaderTransformer.h2RequestPath(h2);
        HttpHeaders h1Headers = HeaderTransformer.h2RequestToH1Headers(h2);

        DefaultHttpRequest request = new DefaultHttpRequest(HttpVersion.HTTP_1_1, method, path, h1Headers);
        HeaderTransformer.addVia(request.headers(), ProtocolVersion.HTTP_2);
        return request;
    }

    /**
     * Translates an HTTP/2 response HEADERS frame to an HTTP/1.1 response.
     *
     * @param headersFrame the HTTP/2 response HEADERS frame
     * @return the HTTP/1.1 response
     */
    public HttpResponse translateResponse(Http2HeadersFrame headersFrame) {
        Http2Headers h2 = headersFrame.headers();

        int statusCode = HeaderTransformer.h2ResponseStatus(h2);
        HttpHeaders h1Headers = HeaderTransformer.h2ResponseToH1Headers(h2);

        DefaultHttpResponse response = new DefaultHttpResponse(
                HttpVersion.HTTP_1_1, HttpResponseStatus.valueOf(statusCode), h1Headers);
        HeaderTransformer.addVia(response.headers(), ProtocolVersion.HTTP_2);
        return response;
    }

    /**
     * Translates an HTTP/2 DATA frame to an HTTP/1.1 body chunk.
     *
     * <p>If endStream is true, returns a {@link LastHttpContent}. Otherwise
     * returns a regular {@link HttpContent}.</p>
     *
     * @param dataFrame the HTTP/2 DATA frame
     * @return HTTP/1.1 body chunk
     */
    public HttpContent translateBody(Http2DataFrame dataFrame) {
        ByteBuf content = dataFrame.content().retain();
        if (dataFrame.isEndStream()) {
            return new DefaultLastHttpContent(content);
        }
        return new DefaultHttpContent(content);
    }

    /**
     * Translates HTTP/2 trailing HEADERS frame to HTTP/1.1 LastHttpContent with trailers.
     *
     * <p>Per RFC 9112 Section 7.1.2, HTTP/1.1 trailers are sent after the last chunk
     * in chunked transfer encoding. The caller must ensure the response uses chunked
     * encoding when trailers are present.</p>
     *
     * @param headersFrame the HTTP/2 trailing HEADERS frame (endStream=true)
     * @return LastHttpContent with trailing headers
     */
    public LastHttpContent translateTrailers(Http2HeadersFrame headersFrame) {
        HttpHeaders h1Trailers = HeaderTransformer.h2TrailersToH1(headersFrame.headers());
        DefaultLastHttpContent lastContent = new DefaultLastHttpContent();
        lastContent.trailingHeaders().add(h1Trailers);
        return lastContent;
    }

    /**
     * Creates an empty LastHttpContent to signal end-of-message when the HTTP/2
     * HEADERS or DATA frame has endStream=true but no trailing headers.
     *
     * @return empty LastHttpContent
     */
    public LastHttpContent endOfMessage() {
        return LastHttpContent.EMPTY_LAST_CONTENT;
    }
}
