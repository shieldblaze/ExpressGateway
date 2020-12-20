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

import com.shieldblaze.expressgateway.protocol.http.adapter.http1.HTTPOutboundAdapter;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.http.CustomFullHttpResponse;
import io.netty.handler.codec.http.CustomHttpContent;
import io.netty.handler.codec.http.CustomHttpRequest;
import io.netty.handler.codec.http.CustomHttpResponse;
import io.netty.handler.codec.http.CustomLastHttpContent;
import io.netty.handler.codec.http.HttpContent;
import io.netty.handler.codec.http.HttpFrame;
import io.netty.handler.codec.http.HttpMethod;
import io.netty.handler.codec.http.HttpRequest;
import io.netty.handler.codec.http.HttpResponse;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.netty.handler.codec.http.HttpVersion;
import org.junit.jupiter.api.Test;

import java.util.Random;

import static org.junit.jupiter.api.Assertions.*;

class HTTPOutboundAdapterTest {

    @Test
    void simpleGETRequestAndResponseTest() {
        EmbeddedChannel embeddedChannel = new EmbeddedChannel(new HTTPOutboundAdapter());

        CustomHttpRequest httpRequest = new CustomHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.GET, "/", HttpFrame.Protocol.HTTP_1_1, 1);
        embeddedChannel.writeOutbound(httpRequest);
        embeddedChannel.flushOutbound();

        HttpResponse httpResponse = new CustomHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK, HttpFrame.Protocol.HTTP_1_1, 1);
        embeddedChannel.writeInbound(httpResponse);
        embeddedChannel.flushInbound();

        assertEquals(1, httpRequest.id());

        embeddedChannel.readInbound();

        final int numBytes = 1024 * 100;
        for (int i = 1; i <= numBytes; i++) {
            ByteBuf byteBuf = Unpooled.wrappedBuffer(("Meow" + i).getBytes());
            HttpContent httpContent;
            if (i == numBytes) {
                httpContent = new CustomLastHttpContent(byteBuf, HttpFrame.Protocol.HTTP_1_1, 1);
            } else {
                httpContent = new CustomHttpContent(byteBuf, HttpFrame.Protocol.HTTP_1_1, 1);
            }
            embeddedChannel.writeInbound(httpContent);
            embeddedChannel.flushInbound();

            CustomHttpContent responseHttpContent = embeddedChannel.readInbound();
            assertEquals("Meow" + i, new String(ByteBufUtil.getBytes(byteBuf)));
            assertEquals(1, responseHttpContent.id());
            responseHttpContent.release();
        }

        embeddedChannel.close();
    }

    @Test
    void chunkedPOSTRequestAndResponse() {
        EmbeddedChannel embeddedChannel = new EmbeddedChannel(new HTTPOutboundAdapter());

        HttpRequest httpRequest = new CustomHttpRequest(HttpVersion.HTTP_1_1, HttpMethod.POST, "/", HttpFrame.Protocol.HTTP_1_1, 1);
        embeddedChannel.writeOutbound(httpRequest);
        embeddedChannel.flushOutbound();

        HttpResponse httpResponse = new CustomFullHttpResponse(HttpVersion.HTTP_1_1, HttpResponseStatus.OK,
                embeddedChannel.alloc().buffer(), HttpFrame.Protocol.HTTP_1_1, 1);
        embeddedChannel.writeOutbound(httpResponse);
        embeddedChannel.flushOutbound();

        final int numBytes = 1024 * 100;
        for (int i = 1; i <= numBytes; i++) {
            byte[] bytes = new byte[1];
            new Random().nextBytes(bytes);

            ByteBuf byteBuf = Unpooled.wrappedBuffer(("Meow" + i).getBytes());
            HttpContent httpContent;
            if (i == numBytes) {
                httpContent = new CustomLastHttpContent(byteBuf, HttpFrame.Protocol.HTTP_1_1, 1);
            } else {
                httpContent = new CustomHttpContent(byteBuf, HttpFrame.Protocol.HTTP_1_1, 1);
            }
            embeddedChannel.writeInbound(httpContent);
            embeddedChannel.flushInbound();

            CustomHttpContent responseHttpContent = embeddedChannel.readInbound();
            assertEquals("Meow" + i, new String(ByteBufUtil.getBytes(byteBuf)));
            assertEquals(1, responseHttpContent.id());
            responseHttpContent.release();
        }

        embeddedChannel.close();
    }
}
