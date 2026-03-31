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
package com.shieldblaze.expressgateway.protocol.http;

import com.shieldblaze.expressgateway.common.annotation.NonNull;
import com.shieldblaze.expressgateway.protocol.http.grpc.GrpcAwareCompressorEncoder;
import io.netty.handler.codec.compression.CompressionOptions;
import io.netty.util.AsciiString;
import io.netty.handler.codec.http2.CompressorHttp2ConnectionEncoder;
import io.netty.handler.codec.http2.DefaultHttp2Connection;
import io.netty.handler.codec.http2.DefaultHttp2ConnectionDecoder;
import io.netty.handler.codec.http2.DefaultHttp2ConnectionEncoder;
import io.netty.handler.codec.http2.DefaultHttp2FrameReader;
import io.netty.handler.codec.http2.DefaultHttp2FrameWriter;
import io.netty.handler.codec.http2.DefaultHttp2HeadersDecoder;
import io.netty.handler.codec.http2.DelegatingDecompressorFrameListener;
import io.netty.handler.codec.http2.Http2Connection;
import io.netty.handler.codec.http2.Http2ConnectionDecoder;
import io.netty.handler.codec.http2.Http2ConnectionEncoder;
import io.netty.handler.codec.http2.Http2FrameCodec;
import io.netty.handler.codec.http2.Http2FrameCodecBuilder;
import io.netty.handler.codec.http2.Http2FrameReader;
import io.netty.handler.codec.http2.Http2FrameWriter;
import io.netty.handler.codec.http2.Http2HeadersEncoder;
import io.netty.handler.codec.http2.Http2PromisedRequestVerifier;
import io.netty.handler.codec.http2.Http2Settings;

import static io.netty.handler.codec.http2.Http2CodecUtil.DEFAULT_HEADER_LIST_SIZE;

public final class CompressibleHttp2FrameCodec extends Http2FrameCodecBuilder {

    private final boolean isServer;
    private final CompressionOptions[] compressionOptions;

    /**
     * F-14: Operator-configurable max header list size. When set to a positive value,
     * this overrides any value from initialSettings. When zero or negative (default),
     * the value from initialSettings (or Netty's DEFAULT_HEADER_LIST_SIZE) is used.
     */
    private long configuredMaxHeaderListSize;

    public static CompressibleHttp2FrameCodec forServer(CompressionOptions[] compressionOptions) {
        return new CompressibleHttp2FrameCodec(true, compressionOptions);
    }

    public static CompressibleHttp2FrameCodec forClient(CompressionOptions[] compressionOptions) {
        return new CompressibleHttp2FrameCodec(false, compressionOptions);
    }

    @NonNull
    public CompressibleHttp2FrameCodec(boolean isServer, CompressionOptions[] compressionOptions) {
        this.isServer = isServer;
        this.compressionOptions = compressionOptions.clone();
    }

    /**
     * F-14: Set the maximum header list size for the HPACK decoder.
     * Per RFC 9113 Section 6.5.2 (SETTINGS_MAX_HEADER_LIST_SIZE), this controls
     * the maximum size of header block fragments the decoder will accept.
     *
     * @param maxHeaderListSize maximum header list size in bytes; must be positive
     * @return this builder for chaining
     */
    public CompressibleHttp2FrameCodec maxHeaderListSize(long maxHeaderListSize) {
        this.configuredMaxHeaderListSize = maxHeaderListSize;
        return this;
    }

    @Override
    protected Http2FrameCodec build(Http2ConnectionDecoder decoder, Http2ConnectionEncoder encoder, Http2Settings initialSettings) {
        autoAckPingFrame(true);
        // RFC 9113 Section 6.5: SETTINGS_ACK handling.
        //
        // autoAckSettingsFrame(true) instructs Netty's Http2FrameCodec to automatically
        // send SETTINGS_ACK frames in response to received SETTINGS frames. This ensures
        // compliance with RFC 9113 Section 6.5.3:
        //   "The SETTINGS frame always applies to a connection, never a single stream.
        //    [...] The peer acknowledges receipt of SETTINGS by sending a SETTINGS frame
        //    with the ACK flag set."
        //
        // Netty also handles the outbound side: when we send SETTINGS, Netty's
        // Http2ConnectionHandler tracks a pending SETTINGS_ACK from the peer. If the
        // peer does not acknowledge within the codec's graceful shutdown timeout (30s
        // default, configurable via Http2ConnectionHandler.gracefulShutdownTimeoutMillis),
        // the connection is closed. This satisfies RFC 9113 Section 6.5.3:
        //   "A receiver of a SETTINGS frame MAY choose to close the connection
        //    if it has not received a SETTINGS_ACK within a reasonable time."
        //
        // No additional application-level SETTINGS_ACK timeout is needed.
        autoAckSettingsFrame(true);
        flushPreface(true);

        // F-14: Prefer the explicitly configured value over the settings value.
        // This allows operators to control maxHeaderListSize via HttpConfiguration
        // without embedding it in the Http2Settings (which is SETTINGS wire-level).
        long maxHeaderListSize;
        if (configuredMaxHeaderListSize > 0) {
            maxHeaderListSize = configuredMaxHeaderListSize;
        } else {
            Long fromSettings = initialSettings.maxHeaderListSize();
            maxHeaderListSize = fromSettings != null ? fromSettings : DEFAULT_HEADER_LIST_SIZE;
        }

        Http2Connection connection = new DefaultHttp2Connection(isServer);

        Http2FrameReader reader = new DefaultHttp2FrameReader(new DefaultHttp2HeadersDecoder(true, maxHeaderListSize));
        Http2FrameWriter writer = new DefaultHttp2FrameWriter(
                (name, value) -> {
                    // Mark security-sensitive headers as sensitive to prevent them
                    // from being added to the HPACK dynamic table.
                    // Per RFC 7541 Section 7.1.3: these headers SHOULD use literal
                    // encoding without indexing to avoid leaking credentials.
                    return AsciiString.contentEqualsIgnoreCase(name, "authorization")
                            || AsciiString.contentEqualsIgnoreCase(name, "proxy-authorization")
                            || AsciiString.contentEqualsIgnoreCase(name, "cookie")
                            || AsciiString.contentEqualsIgnoreCase(name, "set-cookie")
                            || AsciiString.contentEqualsIgnoreCase(name, "www-authenticate")
                            || AsciiString.contentEqualsIgnoreCase(name, "proxy-authenticate");
                }, false);

        Http2ConnectionEncoder rawEncoder = new DefaultHttp2ConnectionEncoder(connection, writer);
        Http2ConnectionEncoder compressorEncoder = new CompressorHttp2ConnectionEncoder(rawEncoder, compressionOptions);
        encoder = new GrpcAwareCompressorEncoder(compressorEncoder, rawEncoder);
        decoder = new DefaultHttp2ConnectionDecoder(connection, encoder, reader, Http2PromisedRequestVerifier.ALWAYS_VERIFY, true, true, true);

        Http2FrameCodec http2FrameCodec = super.build(decoder, encoder, initialSettings);
        // maxAllocation=0 means no limit on decompressed size, matching the previous default behavior.
        decoder.frameListener(new DelegatingDecompressorFrameListener(connection, decoder.frameListener(), 0));
        return http2FrameCodec;
    }
}
