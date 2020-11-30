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

package com.shieldblaze.expressgateway.protocol.http.adapter.http1;

import com.shieldblaze.expressgateway.protocol.http.Headers;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.DefaultFullHttpRequest;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultHttpRequest;
import io.netty.handler.codec.http.DefaultHttpResponse;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpHeaderNames;
import io.netty.handler.codec.http.HttpHeaderValues;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.Http2TranslatedHttpContent;
import io.netty.handler.codec.http2.HttpConversionUtil;
import org.junit.jupiter.api.Test;

import java.util.Random;

import static org.junit.jupiter.api.Assertions.*;

class HTTPOutboundAdapterTest {

    @Test
    void simpleGETRequestAndResponseTest() {
        EmbeddedChannel embeddedChannel = new EmbeddedChannel(new HTTPOutboundAdapter());

        HttpRequest httpRequest = new DefaultFullHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
        httpRequest.headers().set(Headers.STREAM_HASH, 1);
        httpRequest.headers().set(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text(), "https");
        embeddedChannel.writeOutbound(httpRequest);
        embeddedChannel.flushOutbound();

        HttpRequest transformedRequest = embeddedChannel.readOutbound();
        assertFalse(transformedRequest.headers().contains(Headers.STREAM_HASH));
        assertFalse(transformedRequest.headers().contains(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text()));

        HttpResponse httpResponse = new DefaultHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
        embeddedChannel.writeInbound(httpResponse);
        embeddedChannel.flushInbound();

        HttpResponse transformedResponse = embeddedChannel.readInbound();
        assertEquals(1, Long.parseLong(transformedResponse.headers().get(Headers.STREAM_HASH)));

        final int numBytes = 1024 * 100;
        for (int i = 1; i <= numBytes; i++) {
            ByteBuf byteBuf = Unpooled.wrappedBuffer(("Meow" + i).getBytes());
            HttpContent httpContent;
            if (i == numBytes) {
                httpContent = new DefaultLastHttpContent(byteBuf);
            } else {
                httpContent = new DefaultHttpContent(byteBuf);
            }
            embeddedChannel.writeInbound(httpContent);
            embeddedChannel.flushInbound();

            Http2TranslatedHttpContent responseHttpContent = embeddedChannel.readInbound();
            assertEquals("Meow" + i, new String(ByteBufUtil.getBytes(byteBuf)));
            assertEquals(1, responseHttpContent.streamHash());
            responseHttpContent.release();
        }

        embeddedChannel.close();
    }

    @Test
    void chunkedPOSTRequestAndResponse() {
        EmbeddedChannel embeddedChannel = new EmbeddedChannel(new HTTPOutboundAdapter());

        HttpRequest httpRequest = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.POST, "/");
        httpRequest.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
        httpRequest.headers().set(Headers.STREAM_HASH, 1);
        embeddedChannel.writeOutbound(httpRequest);
        embeddedChannel.writeOutbound();

        HttpRequest transformedRequest = embeddedChannel.readOutbound();
        assertFalse(transformedRequest.headers().contains(Headers.STREAM_HASH));
        assertFalse(transformedRequest.headers().contains(HttpConversionUtil.ExtensionHeaderNames.SCHEME.text()));

        HttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();

        final int numBytes = 1024 * 100;
        for (int i = 1; i <= numBytes; i++) {
            byte[] bytes = new byte[1];
            new Random().nextBytes(bytes);

            ByteBuf byteBuf = Unpooled.wrappedBuffer(("Meow" + i).getBytes());
            HttpContent httpContent;
            if (i == numBytes) {
                httpContent = new DefaultLastHttpContent(byteBuf);
            } else {
                httpContent = new DefaultHttpContent(byteBuf);
            }
            embeddedChannel.writeInbound(httpContent);
            embeddedChannel.flushInbound();

            Http2TranslatedHttpContent responseHttpContent = embeddedChannel.readInbound();
            assertEquals("Meow" + i, new String(ByteBufUtil.getBytes(byteBuf)));
            assertEquals(1, responseHttpContent.streamHash());
            responseHttpContent.release();
        }

        embeddedChannel.close();
    }
}
