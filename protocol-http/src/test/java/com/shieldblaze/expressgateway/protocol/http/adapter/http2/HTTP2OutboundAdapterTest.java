/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.protocol.http.adapter.http2;

import com.shieldblaze.expressgateway.protocol.http.Headers;
import com.shieldblaze.expressgateway.protocol.http.compression.HTTP2ContentDecompressor;
import io.netty.channel.ChannelDuplexHandler;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.ChannelPromise;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.DefaultFullHttpRequest;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.DefaultHttp2Connection;
import io.netty.handler.codec.http2.DefaultHttp2ConnectionDecoder;
import io.netty.handler.codec.http2.DefaultHttp2ConnectionEncoder;
import io.netty.handler.codec.http2.DefaultHttp2FrameReader;
import io.netty.handler.codec.http2.DefaultHttp2FrameWriter;
import io.netty.handler.codec.http2.DefaultHttp2HeadersDecoder;
import io.netty.handler.codec.http2.Http2Connection;
import io.netty.handler.codec.http2.Http2ConnectionEncoder;
import io.netty.handler.codec.http2.Http2FrameCodec;
import io.netty.handler.codec.http2.Http2FrameReader;
import io.netty.handler.codec.http2.Http2FrameWriter;
import io.netty.handler.codec.http2.Http2HeadersEncoder;
import io.netty.handler.codec.http2.Http2HeadersFrame;
import io.netty.handler.codec.http2.Http2PromisedRequestVerifier;
import io.netty.handler.codec.http2.Http2Settings;
import io.netty.handler.codec.http2.HttpConversionUtil;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertTrue;

class HTTP2OutboundAdapterTest {

    @Test
    void simpleGETRequestAndResponse() throws InterruptedException {
        EmbeddedChannel embeddedChannel = new EmbeddedChannel(
                newCodec(),
                new ChannelDuplexHandler() {
                    @Override
                    public void write(ChannelHandlerContext ctx, Object msg, ChannelPromise promise) {
                        if (msg instanceof Http2HeadersFrame) {
                            Http2HeadersFrame headersFrame = (Http2HeadersFrame) msg;
                            assertTrue(headersFrame.isEndStream());
                            return;
                        }

                        throw new IllegalArgumentException("Unknown Object: " + msg);
                    }
                },
                new HTTP2OutboundAdapter());

        HttpRequest httpRequest = new DefaultFullHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
        httpRequest.headers().set(Headers.STREAM_HASH, 1);
        httpRequest.headers().set(Headers.X_FORWARDED_HTTP_VERSION, Headers.Values.HTTP_2);
        httpRequest.headers().set(HttpConversionUtil.ExtensionHeaderNames.STREAM_ID.text(), 2);
        httpRequest.headers().set(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text(), "https");
        embeddedChannel.writeOutbound(httpRequest);
        embeddedChannel.flushOutbound();

        Thread.sleep(1000L);

        embeddedChannel.close();
    }

    Http2FrameCodec newCodec() {
        Http2Settings http2Settings = Http2Settings.defaultSettings();
        Http2Connection connection = new DefaultHttp2Connection(false);

        Http2FrameReader reader = new DefaultHttp2FrameReader(new DefaultHttp2HeadersDecoder(true, http2Settings.maxHeaderListSize()));
        Http2FrameWriter writer = new DefaultHttp2FrameWriter(Http2HeadersEncoder.NEVER_SENSITIVE, false);

        Http2ConnectionEncoder encoder = new DefaultHttp2ConnectionEncoder(connection, writer);
        DefaultHttp2ConnectionDecoder decoder = new DefaultHttp2ConnectionDecoder(connection, encoder, reader, Http2PromisedRequestVerifier.ALWAYS_VERIFY,
                true, true);

        Http2FrameCodec http2FrameCodec = new Http2FrameCodec(encoder, decoder, http2Settings, false);
        decoder.frameListener(new HTTP2ContentDecompressor(connection, decoder.frameListener()));
        return http2FrameCodec;
    }
}
