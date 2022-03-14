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
package com.shieldblaze.expressgateway.intercommunication;

import com.shieldblaze.expressgateway.intercommunication.messages.request.DeleteDataRequest;
import com.shieldblaze.expressgateway.intercommunication.messages.request.MemberJoinRequest;
import com.shieldblaze.expressgateway.intercommunication.messages.request.MemberLeaveRequest;
import com.shieldblaze.expressgateway.intercommunication.messages.request.UpsertDataRequest;
import com.shieldblaze.expressgateway.intercommunication.messages.response.DeleteDataResponse;
import com.shieldblaze.expressgateway.intercommunication.messages.response.MemberJoinResponse;
import com.shieldblaze.expressgateway.intercommunication.messages.response.MemberLeaveResponse;
import com.shieldblaze.expressgateway.intercommunication.messages.response.UpsertDataResponse;
import io.netty.buffer.ByteBuf;
import io.netty.buffer.ByteBufUtil;
import io.netty.buffer.Unpooled;
import io.netty.channel.embedded.EmbeddedChannel;
import io.netty.handler.codec.DelimiterBasedFrameDecoder;
import org.junit.jupiter.api.Test;

import java.net.InetSocketAddress;
import java.net.SocketAddress;
import java.nio.charset.StandardCharsets;
import java.security.SecureRandom;
import java.util.ArrayList;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class EncoderTest {

    @Test
    void memberJoinRequestTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        MemberJoinRequest request = new MemberJoinRequest(Unpooled.wrappedBuffer(id));
        embeddedChannel.writeOutbound(request);

        ByteBuf firstBuf = embeddedChannel.readOutbound();
        ByteBuf dupBuf = firstBuf.duplicate();

        assertEquals(Messages.MAGIC().readInt(), dupBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(dupBuf.readBytes(16)));
        assertEquals(Messages.JOIN_REQUEST().readShort(), dupBuf.readShort());
        assertEquals(Messages.DELIMITER().readInt(), dupBuf.readInt());

        embeddedChannel.writeInbound(firstBuf);
        request = embeddedChannel.readInbound();

        assertArrayEquals(id, ByteBufUtil.getBytes(request.id()));
        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void memberJoinResponseTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        MemberJoinResponse response = new MemberJoinResponse(Unpooled.wrappedBuffer(id));
        embeddedChannel.writeOutbound(response);

        ByteBuf firstBuf = embeddedChannel.readOutbound();
        ByteBuf dupBuf = firstBuf.duplicate();

        assertEquals(Messages.MAGIC().readInt(), dupBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(dupBuf.readBytes(16)));
        assertEquals(Messages.JOIN_RESPONSE().readShort(), dupBuf.readShort());
        assertEquals(Messages.DELIMITER().readInt(), dupBuf.readInt());

        embeddedChannel.writeInbound(firstBuf);
        response = embeddedChannel.readInbound();

        assertArrayEquals(id, ByteBufUtil.getBytes(response.id()));
        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void memberLeaveRequestTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        MemberLeaveRequest request = new MemberLeaveRequest(Unpooled.wrappedBuffer(id));
        embeddedChannel.writeOutbound(request);

        ByteBuf firstBuf = embeddedChannel.readOutbound();
        ByteBuf dupBuf = firstBuf.duplicate();

        assertEquals(Messages.MAGIC().readInt(), dupBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(dupBuf.readBytes(16)));
        assertEquals(Messages.LEAVE_REQUEST().readShort(), dupBuf.readShort());
        assertEquals(Messages.DELIMITER().readInt(), dupBuf.readInt());

        embeddedChannel.writeInbound(firstBuf);
        request = embeddedChannel.readInbound();

        assertArrayEquals(id, ByteBufUtil.getBytes(request.id()));
        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void memberLeaveResponseTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        MemberLeaveResponse response = new MemberLeaveResponse(Unpooled.wrappedBuffer(id));
        embeddedChannel.writeOutbound(response);

        ByteBuf firstBuf = embeddedChannel.readOutbound();
        ByteBuf dupBuf = firstBuf.duplicate();

        assertEquals(Messages.MAGIC().readInt(), dupBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(dupBuf.readBytes(16)));
        assertEquals(Messages.LEAVE_RESPONSE().readShort(), dupBuf.readShort());
        assertEquals(Messages.DELIMITER().readInt(), dupBuf.readInt());

        embeddedChannel.writeInbound(firstBuf);
        response = embeddedChannel.readInbound();

        assertArrayEquals(id, ByteBufUtil.getBytes(response.id()));
        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void upsertDataRequestTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        List<KeyValuePair> keyValuePairs = new ArrayList<>();
        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;
            String value = "MeowMeow" + i;

            keyValuePairs.add(new KeyValuePair(key, value));
        }

        UpsertDataRequest request = new UpsertDataRequest(Unpooled.wrappedBuffer(id), keyValuePairs);
        embeddedChannel.writeOutbound(request);

        ByteBuf byteBuf = embeddedChannel.readOutbound();
        ByteBuf dupBuf = byteBuf.duplicate();
        assertEquals(Messages.MAGIC().readInt(), byteBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(byteBuf.readBytes(16)));
        assertEquals(Messages.UPSET_DATA_REQUEST().readShort(), byteBuf.readShort());
        assertEquals(100, byteBuf.readInt());

        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;
            String value = "MeowMeow" + i;

            int keyLength = byteBuf.readInt();
            String _key = byteBuf.readBytes(keyLength).toString(StandardCharsets.UTF_8);

            int valueLength = byteBuf.readInt();
            String _value = byteBuf.readBytes(valueLength).toString(StandardCharsets.UTF_8);

            assertEquals(key, _key);
            assertEquals(value, _value);
        }
        assertEquals(Messages.DELIMITER().readInt(), byteBuf.readInt());

        embeddedChannel.writeInbound(dupBuf);
        request = embeddedChannel.readInbound();

        assertArrayEquals(id, ByteBufUtil.getBytes(request.id()));
        assertEquals(100, request.keyValuePairs().size());

        int count = 0;
        for (KeyValuePair keyValuePair : request.keyValuePairs()) {
            assertEquals("Cat" + count, keyValuePair.key());
            assertEquals("MeowMeow" + count, keyValuePair.value());
            count++;
        }

        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void upsertDataResponseTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        List<KeyValuePair> keyValuePairs = new ArrayList<>();
        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;

            keyValuePairs.add(new KeyValuePair(key));
        }

        UpsertDataResponse response = new UpsertDataResponse(Unpooled.wrappedBuffer(id), keyValuePairs);
        embeddedChannel.writeOutbound(response);

        ByteBuf byteBuf = embeddedChannel.readOutbound();
        ByteBuf dupBuf = byteBuf.duplicate();
        assertEquals(Messages.MAGIC().readInt(), byteBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(byteBuf.readBytes(16)));
        assertEquals(Messages.UPSET_DATA_RESPONSE().readShort(), byteBuf.readShort());
        assertEquals(100, byteBuf.readInt());

        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;

            int keyLength = byteBuf.readInt();
            String _key = byteBuf.readBytes(keyLength).toString(StandardCharsets.UTF_8);

            assertEquals(key, _key);
        }
        assertEquals(Messages.DELIMITER().readInt(), byteBuf.readInt());

        embeddedChannel.writeInbound(dupBuf);
        response = embeddedChannel.readInbound();

        assertArrayEquals(id, ByteBufUtil.getBytes(response.id()));
        assertEquals(100, response.keyValuePairs().size());

        int count = 0;
        for (KeyValuePair keyValuePair : response.keyValuePairs()) {
            assertEquals("Cat" + count, keyValuePair.key());
            count++;
        }

        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void deleteDataRequestTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        List<KeyValuePair> keyValuePairs = new ArrayList<>();
        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;

            keyValuePairs.add(new KeyValuePair(key));
        }

        DeleteDataRequest request = new DeleteDataRequest(Unpooled.wrappedBuffer(id), keyValuePairs);
        embeddedChannel.writeOutbound(request);

        ByteBuf byteBuf = embeddedChannel.readOutbound();
        ByteBuf dupBuf = byteBuf.duplicate();
        assertEquals(Messages.MAGIC().readInt(), byteBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(byteBuf.readBytes(16)));
        assertEquals(Messages.DELETE_DATA_REQUEST().readShort(), byteBuf.readShort());
        assertEquals(100, byteBuf.readInt());

        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;

            int keyLength = byteBuf.readInt();
            String _key = byteBuf.readBytes(keyLength).toString(StandardCharsets.UTF_8);

            assertEquals(key, _key);
        }
        assertEquals(Messages.DELIMITER().readInt(), byteBuf.readInt());

        embeddedChannel.writeInbound(dupBuf);
        request = embeddedChannel.readInbound();

        assertArrayEquals(id, ByteBufUtil.getBytes(request.id()));
        assertEquals(100, request.keyValuePairs().size());

        int count = 0;
        for (KeyValuePair keyValuePair : request.keyValuePairs()) {
            assertEquals("Cat" + count, keyValuePair.key());
            count++;
        }

        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void deleteDataResponseTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        List<KeyValuePair> keyValuePairs = new ArrayList<>();
        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;

            keyValuePairs.add(new KeyValuePair(key));
        }

        DeleteDataResponse response = new DeleteDataResponse(Unpooled.wrappedBuffer(id), keyValuePairs);
        embeddedChannel.writeOutbound(response);

        ByteBuf byteBuf = embeddedChannel.readOutbound();
        ByteBuf dupBuf = byteBuf.duplicate();
        assertEquals(Messages.MAGIC().readInt(), byteBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(byteBuf.readBytes(16)));
        assertEquals(Messages.DELETE_DATA_RESPONSE().readShort(), byteBuf.readShort());
        assertEquals(100, byteBuf.readInt());

        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;

            int keyLength = byteBuf.readInt();
            String _key = byteBuf.readBytes(keyLength).toString(StandardCharsets.UTF_8);

            assertEquals(key, _key);
        }

        assertEquals(Messages.DELIMITER().readInt(), byteBuf.readInt());

        embeddedChannel.writeInbound(dupBuf);
        response = embeddedChannel.readInbound();

        assertArrayEquals(id, ByteBufUtil.getBytes(response.id()));
        assertEquals(100, response.keyValuePairs().size());

        int count = 0;
        for (KeyValuePair keyValuePair : response.keyValuePairs()) {
            assertEquals("Cat" + count, keyValuePair.key());
            count++;
        }

        assertTrue(embeddedChannel.close().isSuccess());
    }

    private EmbeddedChannel newEmbeddedChannel() {
        return new EmbeddedChannel(new DelimiterBasedFrameDecoder(10_000_000, Messages.DELIMITER()), new Encoder(), new Decoder()) {
            @Override
            protected SocketAddress remoteAddress0() {
                return new InetSocketAddress("127.0.0.1", 9110);
            }
        };
    }
}
