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
import io.netty.handler.codec.http.HttpResponseDecoder;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import io.netty.handler.codec.http2.Http2TranslatedHttpContent;
import org.junit.jupiter.api.Test;

import java.util.Random;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class HTTPInboundAdapterTest {

    @Test
    void simpleGETRequest() {
        EmbeddedChannel embeddedChannel = new EmbeddedChannel(new HTTPInboundAdapter());

        HttpRequest httpRequest = new DefaultFullHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/");
        embeddedChannel.writeInbound(httpRequest);
        embeddedChannel.flushInbound();

        HttpRequest responseRequest = embeddedChannel.readInbound();
        assertTrue(responseRequest.headers().contains(Headers.STREAM_HASH));

        embeddedChannel.close();
    }

    @Test
    void simplePOSTRequestAndResponse() {
        EmbeddedChannel embeddedChannel = new EmbeddedChannel(new HTTPInboundAdapter());

        HttpRequest httpRequest = new DefaultHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.POST, "/");
        httpRequest.headers().set(HttpHeaderNames.TRANSFER_ENCODING, HttpHeaderValues.CHUNKED);
        embeddedChannel.writeInbound(httpRequest);
        embeddedChannel.flushInbound();

        HttpRequest transformedRequest = embeddedChannel.readInbound();
        assertEquals(HttpHeaderValues.CHUNKED.toString(), transformedRequest.headers().get(HttpHeaderNames.TRANSFER_ENCODING));
        long streamHash = Long.parseLong(transformedRequest.headers().get(Headers.STREAM_HASH));

        final int numBytes = 1024 * 1000;

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
            assertEquals(streamHash, responseHttpContent.streamHash());
            responseHttpContent.release();
        }

        HttpResponse httpResponse = new DefaultFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK);
        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();

        HttpResponse transformedResponse = embeddedChannel.readOutbound();
        assertFalse(transformedResponse.headers().contains(Headers.STREAM_HASH));

        embeddedChannel.close();
    }
}
