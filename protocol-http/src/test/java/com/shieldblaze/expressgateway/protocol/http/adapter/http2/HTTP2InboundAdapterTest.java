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

import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.FullHttpRequest;
import io.netty.handler.codec.http2.DefaultHttp2Headers;
import io.netty.handler.codec.http2.DefaultHttp2HeadersFrame;
import io.netty.handler.codec.http2.Http2HeadersFrame;

import io.netty.handler.codec.http2.HttpConversionUtil;
import org.junit.jupiter.api.AfterAll;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

class HTTP2InboundAdapterTest {

    static EmbeddedChannel embeddedChannel;

    @BeforeAll
    static void setupEmbeddedChannel() {
        embeddedChannel = new EmbeddedChannel(new HTTP2InboundAdapter());
    }

    @AfterAll
    static void shutdownEmbeddedChannel() {
        embeddedChannel.close();
    }

    @Test
    void testInboundAdapter() {
        Http2HeadersFrame http2HeadersFrame = new DefaultHttp2HeadersFrame(new DefaultHttp2Headers(), true);
        http2HeadersFrame.headers().method("GET");
        http2HeadersFrame.headers().scheme("https");
        http2HeadersFrame.headers().path("/");
        http2HeadersFrame.headers().authority("localhost");
        http2HeadersFrame.stream(new CustomHttp2FrameStream(2));
        embeddedChannel.writeInbound(http2HeadersFrame);
        embeddedChannel.flushInbound();

        FullHttpRequest fullHttpRequest = embeddedChannel.readInbound();

        assertEquals("GET", fullHttpRequest.method().name());
        assertEquals(fullHttpRequest.headers().get("x-http2-scheme"), "https");
        assertEquals("/", fullHttpRequest.uri());
        assertEquals(fullHttpRequest.headers().get("host"), "localhost");
        assertEquals(fullHttpRequest.headers().get("x-http2-stream-id"), String.valueOf(2));
    }
}
