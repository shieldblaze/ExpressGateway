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
import io.netty.handler.codec.http2.DefaultHttp2DataFrame;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2DataFrame;
import io.netty.handler.codec.http2.Http2Headers;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http3.Http3DataFrame;
import io.netty.handler.codec.http3.Http3Headers;
import io.netty.handler.codec.http3.Http3HeadersFrame;

/**
 * Translates HTTP/3 frames to HTTP/2 frames.
 *
 * <h3>Frame mapping (RFC 9114 → RFC 9113)</h3>
 * <ul>
 *   <li>HTTP/3 HEADERS → HTTP/2 HEADERS (QPACK decoded → HPACK encoded by codec)</li>
 *   <li>HTTP/3 DATA → HTTP/2 DATA</li>
 *   <li>QUIC stream FIN → HTTP/2 endStream flag on last DATA or HEADERS frame</li>
 *   <li>QUIC RESET_STREAM → HTTP/2 RST_STREAM</li>
 *   <li>QUIC flow control → HTTP/2 WINDOW_UPDATE (mapped by FlowControlTranslator)</li>
 * </ul>
 *
 * <h3>Connection migration (RFC 9000 Section 9)</h3>
 * HTTP/3 supports connection migration where the client changes IP/port while
 * maintaining the QUIC connection. When translating to HTTP/2 (which is TCP-bound),
 * the backend HTTP/2 connection is unaffected by client-side migration events.
 * The translation layer simply continues forwarding frames from the migrated
 * QUIC connection to the same backend HTTP/2 connection.
 *
 * <h3>Stream independence</h3>
 * HTTP/3 streams are fully independent (no HOL blocking). HTTP/2 streams share
 * one TCP connection and are subject to TCP-level HOL blocking. The translator
 * maps HTTP/3 stream data to HTTP/2 streams on a per-request basis. If the
 * backend HTTP/2 connection experiences TCP congestion, it may block all streams
 * on that connection -- this is an inherent limitation of the H3→H2 translation.
 */
public final class H3ToH2Translator implements ProtocolTranslator {

    static final H3ToH2Translator INSTANCE = new H3ToH2Translator();

    private H3ToH2Translator() {
    }

    @Override
    public ProtocolVersion sourceProtocol() {
        return ProtocolVersion.HTTP_3;
    }

    @Override
    public ProtocolVersion targetProtocol() {
        return ProtocolVersion.HTTP_2;
    }

    /**
     * Translates an HTTP/3 request HEADERS frame to an HTTP/2 HEADERS frame.
     *
     * <p>endStream is set to false because the QUIC stream FIN has not been
     * observed yet at HEADERS time. The caller must set endStream=true on the
     * last DATA frame or send a final empty DATA frame with endStream=true
     * when the QUIC stream closes.</p>
     *
     * @param headersFrame the HTTP/3 request HEADERS frame
     * @param endStream    whether this is the last frame (QUIC FIN already received)
     * @return the HTTP/2 HEADERS frame
     */
    public Http2HeadersFrame translateRequest(Http3HeadersFrame headersFrame, boolean endStream) {
        Http2Headers h2 = HeaderTransformer.h3RequestToH2(headersFrame.headers());
        HeaderTransformer.addVia(h2, ProtocolVersion.HTTP_3);
        return new DefaultHttp2HeadersFrame(h2, endStream);
    }

    /**
     * Translates an HTTP/3 response HEADERS frame to an HTTP/2 HEADERS frame.
     *
     * @param headersFrame the HTTP/3 response HEADERS frame
     * @param endStream    whether this completes the response (no body)
     * @return the HTTP/2 HEADERS frame
     */
    public Http2HeadersFrame translateResponse(Http3HeadersFrame headersFrame, boolean endStream) {
        Http2Headers h2 = HeaderTransformer.h3ResponseToH2(headersFrame.headers());
        HeaderTransformer.addVia(h2, ProtocolVersion.HTTP_3);
        return new DefaultHttp2HeadersFrame(h2, endStream);
    }

    /**
     * Translates an HTTP/3 DATA frame to an HTTP/2 DATA frame.
     *
     * @param dataFrame the HTTP/3 DATA frame
     * @param endStream whether this is the last DATA frame
     * @return the HTTP/2 DATA frame
     */
    public Http2DataFrame translateBody(Http3DataFrame dataFrame, boolean endStream) {
        ByteBuf content = dataFrame.content().retain();
        return new DefaultHttp2DataFrame(content, endStream);
    }

    /**
     * Translates HTTP/3 trailing HEADERS frame to HTTP/2 trailing HEADERS frame.
     *
     * <p>Per RFC 9113 Section 8.1, trailers are always sent with endStream=true.</p>
     *
     * @param headersFrame the HTTP/3 trailing HEADERS frame
     * @return the HTTP/2 trailing HEADERS frame with endStream=true
     */
    public Http2HeadersFrame translateTrailers(Http3HeadersFrame headersFrame) {
        Http2Headers h2Trailers = HeaderTransformer.h3TrailersToH2(headersFrame.headers());
        return new DefaultHttp2HeadersFrame(h2Trailers, true);
    }

    /**
     * Creates an empty HTTP/2 DATA frame with endStream=true to signal end-of-stream
     * when the QUIC stream closes (FIN) without trailing headers.
     *
     * @return HTTP/2 DATA frame with endStream=true and empty content
     */
    public Http2DataFrame endOfStream() {
        return new DefaultHttp2DataFrame(true);
    }
}
