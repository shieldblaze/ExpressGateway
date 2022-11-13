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
import io.netty.handler.codec.compression.CompressionOptions;
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

    public static CompressibleHttp2FrameCodec forServer(CompressionOptions[] compressionOptions) {
        return new CompressibleHttp2FrameCodec(true, compressionOptions);
    }

    public static CompressibleHttp2FrameCodec forClient(CompressionOptions[] compressionOptions) {
        return new CompressibleHttp2FrameCodec(false, compressionOptions);
    }

    @NonNull
    public CompressibleHttp2FrameCodec(boolean isServer, CompressionOptions[] compressionOptions) {
        this.isServer = isServer;
        this.compressionOptions = compressionOptions;
    }

    @Override
    protected Http2FrameCodec build(Http2ConnectionDecoder decoder, Http2ConnectionEncoder encoder, Http2Settings initialSettings) {
        autoAckPingFrame(true);
        autoAckSettingsFrame(true);
        flushPreface(true);

        Long maxHeaderListSize = initialSettings.maxHeaderListSize();
        if (maxHeaderListSize == null) {
            maxHeaderListSize = DEFAULT_HEADER_LIST_SIZE;
        }

        Http2Connection connection = new DefaultHttp2Connection(isServer);

        Http2FrameReader reader = new DefaultHttp2FrameReader(new DefaultHttp2HeadersDecoder(true, maxHeaderListSize));
        Http2FrameWriter writer = new DefaultHttp2FrameWriter(Http2HeadersEncoder.NEVER_SENSITIVE, false);

        encoder = new CompressorHttp2ConnectionEncoder(new DefaultHttp2ConnectionEncoder(connection, writer), compressionOptions);
        decoder = new DefaultHttp2ConnectionDecoder(connection, encoder, reader, Http2PromisedRequestVerifier.ALWAYS_VERIFY, true, true);

        Http2FrameCodec http2FrameCodec = super.build(decoder, encoder, initialSettings);
        decoder.frameListener(new DelegatingDecompressorFrameListener(connection, decoder.frameListener()));
        return http2FrameCodec;
    }
}
