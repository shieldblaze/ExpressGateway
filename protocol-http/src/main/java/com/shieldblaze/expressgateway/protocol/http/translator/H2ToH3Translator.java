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
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http3.DefaultHttp3DataFrame;
import io.netty.handler.codec.http3.DefaultHttp3HeadersFrame;
import io.netty.handler.codec.http3.Http3DataFrame;
import io.netty.handler.codec.http3.Http3Headers;
import io.netty.handler.codec.http3.Http3HeadersFrame;

/**
 * Translates HTTP/2 frames to HTTP/3 frames.
 *
 * <h3>Frame mapping (RFC 9113 → RFC 9114)</h3>
 * <ul>
 *   <li>HTTP/2 HEADERS → HTTP/3 HEADERS (same pseudo-headers, different encoding)</li>
 *   <li>HTTP/2 DATA → HTTP/3 DATA (raw bytes, no framing difference at application level)</li>
 *   <li>HTTP/2 endStream flag → QUIC stream FIN</li>
 *   <li>HTTP/2 RST_STREAM → QUIC stream reset (RESET_STREAM frame)</li>
 *   <li>HTTP/2 WINDOW_UPDATE → not applicable (QUIC handles flow control)</li>
 *   <li>HTTP/2 PING → not applicable (QUIC handles connection liveness)</li>
 *   <li>HTTP/2 GOAWAY → QUIC GOAWAY (different semantics: QUIC uses connection IDs)</li>
 *   <li>HTTP/2 PUSH_PROMISE → not applicable (server push not implemented; deprecated)</li>
 * </ul>
 *
 * <h3>Header encoding</h3>
 * HTTP/2 uses HPACK (RFC 7541), HTTP/3 uses QPACK (RFC 9204). At the logical level,
 * both encode the same set of pseudo-headers and regular headers. The actual encoding
 * is handled by the Netty codecs; this translator operates at the logical level.
 *
 * <h3>Flow control mapping</h3>
 * HTTP/2 has explicit connection+stream-level flow control windows managed via
 * WINDOW_UPDATE. HTTP/3 delegates flow control to QUIC (RFC 9000 Section 4),
 * which provides stream+connection-level flow control at the transport layer.
 * The translation does not need to map window sizes -- it's handled by
 * {@link FlowControlTranslator} propagating backpressure signals.
 *
 * <h3>Stream multiplexing</h3>
 * Both HTTP/2 and HTTP/3 multiplex streams. HTTP/2 streams share one TCP connection
 * (subject to head-of-line blocking at TCP level). HTTP/3 streams are independent
 * QUIC streams with no cross-stream blocking. The translator does not need to handle
 * multiplexing differences -- each stream is translated independently.
 */
public final class H2ToH3Translator implements ProtocolTranslator {

    static final H2ToH3Translator INSTANCE = new H2ToH3Translator();

    private H2ToH3Translator() {
    }

    @Override
    public ProtocolVersion sourceProtocol() {
        return ProtocolVersion.HTTP_2;
    }

    @Override
    public ProtocolVersion targetProtocol() {
        return ProtocolVersion.HTTP_3;
    }

    /**
     * Translates an HTTP/2 request HEADERS frame to an HTTP/3 HEADERS frame.
     *
     * <p>Both protocols share the same pseudo-header set (:method, :path, :scheme,
     * :authority). Connection-specific headers are stripped for defense in depth.</p>
     *
     * @param headersFrame the HTTP/2 request HEADERS frame
     * @return the HTTP/3 HEADERS frame
     */
    public Http3HeadersFrame translateRequest(Http2HeadersFrame headersFrame) {
        Http3Headers h3 = HeaderTransformer.h2RequestToH3(headersFrame.headers());
        HeaderTransformer.addVia(h3, ProtocolVersion.HTTP_2);
        return new DefaultHttp3HeadersFrame(h3);
    }

    /**
     * Translates an HTTP/2 response HEADERS frame to an HTTP/3 HEADERS frame.
     *
     * @param headersFrame the HTTP/2 response HEADERS frame
     * @return the HTTP/3 HEADERS frame
     */
    public Http3HeadersFrame translateResponse(Http2HeadersFrame headersFrame) {
        Http3Headers h3 = HeaderTransformer.h2ResponseToH3(headersFrame.headers());
        HeaderTransformer.addVia(h3, ProtocolVersion.HTTP_2);
        return new DefaultHttp3HeadersFrame(h3);
    }

    /**
     * Translates an HTTP/2 DATA frame to an HTTP/3 DATA frame.
     *
     * <p>The content bytes are forwarded directly. If endStream is true on the H2
     * frame, the caller must close the QUIC stream after writing this frame.</p>
     *
     * @param dataFrame the HTTP/2 DATA frame
     * @return the HTTP/3 DATA frame
     */
    public Http3DataFrame translateBody(Http2DataFrame dataFrame) {
        ByteBuf content = dataFrame.content().retain();
        return new DefaultHttp3DataFrame(content);
    }

    /**
     * Translates HTTP/2 trailing HEADERS frame to HTTP/3 trailing HEADERS frame.
     *
     * @param headersFrame the HTTP/2 trailing HEADERS frame
     * @return the HTTP/3 trailing HEADERS frame
     */
    public Http3HeadersFrame translateTrailers(Http2HeadersFrame headersFrame) {
        Http3Headers h3Trailers = HeaderTransformer.h2TrailersToH3(headersFrame.headers());
        return new DefaultHttp3HeadersFrame(h3Trailers);
    }

    /**
     * Returns whether the HTTP/2 HEADERS frame signals end-of-stream.
     * Used by the caller to determine whether to close the QUIC stream.
     *
     * @param headersFrame the HTTP/2 HEADERS frame
     * @return {@code true} if endStream is set
     */
    public boolean isEndStream(Http2HeadersFrame headersFrame) {
        return headersFrame.isEndStream();
    }

    /**
     * Returns whether the HTTP/2 DATA frame signals end-of-stream.
     *
     * @param dataFrame the HTTP/2 DATA frame
     * @return {@code true} if endStream is set
     */
    public boolean isEndStream(Http2DataFrame dataFrame) {
        return dataFrame.isEndStream();
    }
}
