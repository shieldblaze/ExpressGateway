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
package com.shieldblaze.expressgateway.protocol.http.adapter.http1;

import com.shieldblaze.expressgateway.protocol.http.NonceWrapped;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.DefaultFullHttpRequest;
import io.netty.handler.codec.http.DefaultFullHttpResponse;
import io.netty.handler.codec.http.DefaultHttpContent;
import io.netty.handler.codec.http.DefaultHttpRequest;
import io.netty.handler.codec.http.DefaultHttpResponse;
import io.netty.handler.codec.http.DefaultLastHttpContent;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;

import static io.netty.handler.codec.http.HttpVersion.HTTP_1_1;
import static org.junit.jupiter.api.Assertions.assertEquals;

class HTTPOutboundAdapterTest {

    @Test
    void simpleGETRequestAndResponseTest() {
        EmbeddedChannel embeddedChannel = new EmbeddedChannel(new HTTPOutboundAdapter());

        NonceWrapped<HttpRequest> httpRequest = new NonceWrapped<>(1, new DefaultFullHttpRequest(HTTP_1_1, HttpMethod.GET, "/"));
        embeddedChannel.writeOutbound(httpRequest);
        embeddedChannel.flushOutbound();

        NonceWrapped<HttpResponse> httpResponse = new NonceWrapped<>(1, new DefaultHttpResponse(HTTP_1_1, HttpResponseStatus.OK));
        embeddedChannel.writeInbound(httpResponse);
        embeddedChannel.flushInbound();

        assertEquals(1, httpRequest.nonce());

        embeddedChannel.readInbound();

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

            NonceWrapped<HttpContent> wrappedHttpContent = embeddedChannel.readInbound();
            assertEquals("Meow" + i, byteBuf.toString(StandardCharsets.UTF_8));
            assertEquals(1, wrappedHttpContent.nonce());
            wrappedHttpContent.get().release();
        }

        embeddedChannel.close();
    }

    @Test
    void chunkedPOSTRequestAndResponse() {
        EmbeddedChannel embeddedChannel = new EmbeddedChannel(new HTTPOutboundAdapter());

        NonceWrapped<HttpRequest> httpRequest = new NonceWrapped<>(1, new DefaultHttpRequest(HTTP_1_1, HttpMethod.POST, "/"));
        embeddedChannel.writeOutbound(httpRequest);
        embeddedChannel.flushOutbound();

        NonceWrapped<HttpResponse> httpResponse = new NonceWrapped<>(1, new DefaultFullHttpResponse(HTTP_1_1, HttpResponseStatus.OK, embeddedChannel.alloc().buffer()));
        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();

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

            NonceWrapped<HttpContent> wrappedHttpContent = embeddedChannel.readInbound();
            assertEquals("Meow" + i, byteBuf.toString(StandardCharsets.UTF_8));
            assertEquals(1, wrappedHttpContent.nonce());
            wrappedHttpContent.get().release();
        }

        embeddedChannel.close();
    }
}
