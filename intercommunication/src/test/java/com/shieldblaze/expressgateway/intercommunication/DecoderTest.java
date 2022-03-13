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

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class DecoderTest {

    @Test
    void memberJoinRequestTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        ByteBuf byteBuf = Unpooled.buffer();
        byteBuf.writeInt(Messages.MAGIC);  // Write magic number
        byteBuf.writeBytes(id);            // ID
        byteBuf.writeShort(Messages.JOIN_REQUEST);
        byteBuf.writeInt(Messages.DELIMITER);

        embeddedChannel.writeInbound(byteBuf);
        MemberJoinRequest request = embeddedChannel.readInbound();
        assertEquals(ByteBufUtil.hexDump(id), ByteBufUtil.hexDump(request.id())); // Verify ID

        embeddedChannel.writeOutbound(request);

        byteBuf = embeddedChannel.readOutbound();
        assertEquals(Messages.MAGIC, byteBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(byteBuf.readBytes(16)));
        assertEquals(Messages.JOIN_REQUEST, byteBuf.readShort());
        assertEquals(Messages.DELIMITER, byteBuf.readInt());

        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void memberJoinResponseTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        ByteBuf byteBuf = Unpooled.buffer();
        byteBuf.writeInt(Messages.MAGIC);  // Write magic number
        byteBuf.writeBytes(id);            // ID
        byteBuf.writeShort(Messages.JOIN_RESPONSE);
        byteBuf.writeInt(Messages.DELIMITER);

        embeddedChannel.writeInbound(byteBuf);
        MemberJoinResponse response = embeddedChannel.readInbound();
        assertEquals(ByteBufUtil.hexDump(id), ByteBufUtil.hexDump(response.id())); // Verify ID

        embeddedChannel.writeOutbound(response);

        byteBuf = embeddedChannel.readOutbound();
        assertEquals(Messages.MAGIC, byteBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(byteBuf.readBytes(16)));
        assertEquals(Messages.JOIN_RESPONSE, byteBuf.readShort());
        assertEquals(Messages.DELIMITER, byteBuf.readInt());

        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void memberLeaveRequestTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        ByteBuf byteBuf = Unpooled.buffer();
        byteBuf.writeInt(Messages.MAGIC);   // Write magic number
        byteBuf.writeBytes(id);             // ID
        byteBuf.writeShort(Messages.LEAVE_REQUEST);
        byteBuf.writeInt(Messages.DELIMITER);

        embeddedChannel.writeInbound(byteBuf);

        MemberLeaveRequest request = embeddedChannel.readInbound();
        assertEquals(ByteBufUtil.hexDump(id), ByteBufUtil.hexDump(request.id())); // Verify ID

        embeddedChannel.writeOutbound(request);

        byteBuf = embeddedChannel.readOutbound();
        assertEquals(Messages.MAGIC, byteBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(byteBuf.readBytes(16)));
        assertEquals(Messages.LEAVE_REQUEST, byteBuf.readShort());
        assertEquals(Messages.DELIMITER, byteBuf.readInt());

        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void memberLeaveResponseTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        ByteBuf byteBuf = Unpooled.buffer();
        byteBuf.writeInt(Messages.MAGIC);   // Write magic number
        byteBuf.writeBytes(id);             // ID
        byteBuf.writeShort(Messages.LEAVE_RESPONSE);
        byteBuf.writeInt(Messages.DELIMITER);

        embeddedChannel.writeInbound(byteBuf);

        MemberLeaveResponse response = embeddedChannel.readInbound();
        assertEquals(ByteBufUtil.hexDump(id), ByteBufUtil.hexDump(response.id())); // Verify ID

        embeddedChannel.writeOutbound(response);

        byteBuf = embeddedChannel.readOutbound();
        assertEquals(Messages.MAGIC, byteBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(byteBuf.readBytes(16)));
        assertEquals(Messages.LEAVE_RESPONSE, byteBuf.readShort());
        assertEquals(Messages.DELIMITER, byteBuf.readInt());

        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void upsertDataRequestTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        ByteBuf byteBuf = Unpooled.buffer();
        byteBuf.writeInt(Messages.MAGIC);        // Write magic number
        byteBuf.writeBytes(id);         // ID
        byteBuf.writeShort(Messages.UPSET_DATA_REQUEST);
        byteBuf.writeInt(100);

        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;
            byteBuf.writeInt(key.length());
            ByteBufUtil.writeUtf8(byteBuf, key);

            String value = "MeowMeow" + i;
            byteBuf.writeInt(value.length());
            ByteBufUtil.writeUtf8(byteBuf, value);
        }
        byteBuf.writeInt(Messages.DELIMITER);
        embeddedChannel.writeInbound(byteBuf);

        UpsertDataRequest upsert = embeddedChannel.readInbound();
        assertEquals(ByteBufUtil.hexDump(id), ByteBufUtil.hexDump(upsert.id())); // Verify ID
        assertEquals(100, upsert.keyValuePairs().size());

        int count = 0;
        for (KeyValuePair keyValuePair : upsert.keyValuePairs()) {
            assertEquals("Cat" + count, keyValuePair.key());
            assertEquals("MeowMeow" + count, keyValuePair.value());
            count++;
        }

        embeddedChannel.writeOutbound(upsert);
        byteBuf = embeddedChannel.readOutbound();
        assertEquals(Messages.MAGIC, byteBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(byteBuf.readBytes(16)));
        assertEquals(Messages.UPSET_DATA_REQUEST, byteBuf.readShort());
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

        byteBuf.writeInt(Messages.DELIMITER);
        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void upsertDataResponseTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        ByteBuf byteBuf = Unpooled.buffer();
        byteBuf.writeInt(Messages.MAGIC);        // Write magic number
        byteBuf.writeBytes(id);                  // ID
        byteBuf.writeShort(Messages.UPSET_DATA_RESPONSE);
        byteBuf.writeInt(100);

        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;
            byteBuf.writeInt(key.length());
            ByteBufUtil.writeUtf8(byteBuf, key);
        }

        byteBuf.writeInt(Messages.DELIMITER);
        embeddedChannel.writeInbound(byteBuf);

        UpsertDataResponse response = embeddedChannel.readInbound();
        assertEquals(ByteBufUtil.hexDump(id), ByteBufUtil.hexDump(response.id())); // Verify ID
        assertEquals(100, response.keyValuePairs().size());

        int count = 0;
        for (KeyValuePair keyValuePair : response.keyValuePairs()) {
            assertEquals("Cat" + count, keyValuePair.key());
            count++;
        }

        embeddedChannel.writeOutbound(response);
        byteBuf = embeddedChannel.readOutbound();
        assertEquals(Messages.MAGIC, byteBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(byteBuf.readBytes(16)));
        assertEquals(Messages.UPSET_DATA_RESPONSE, byteBuf.readShort());
        assertEquals(100, byteBuf.readInt());

        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;

            int keyLength = byteBuf.readInt();
            String _key = byteBuf.readBytes(keyLength).toString(StandardCharsets.UTF_8);
            assertEquals(key, _key);
        }

        byteBuf.writeInt(Messages.DELIMITER);
        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void deleteDataRequestTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        ByteBuf byteBuf = Unpooled.buffer();
        byteBuf.writeInt(Messages.MAGIC);        // Write magic number
        byteBuf.writeBytes(id);                  // ID
        byteBuf.writeShort(Messages.DELETE_DATA_REQUEST);
        byteBuf.writeInt(100);

        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;
            byteBuf.writeInt(key.length());
            ByteBufUtil.writeUtf8(byteBuf, key);
        }
        byteBuf.writeInt(Messages.DELIMITER);
        embeddedChannel.writeInbound(byteBuf);

        DeleteDataRequest request = embeddedChannel.readInbound();
        assertEquals(ByteBufUtil.hexDump(id), ByteBufUtil.hexDump(request.id())); // Verify ID
        assertEquals(100, request.keyValuePairs().size());

        int count = 0;
        for (KeyValuePair keyValuePair : request.keyValuePairs()) {
            assertEquals("Cat" + count, keyValuePair.key());
            count++;
        }

        embeddedChannel.writeOutbound(request);
        byteBuf = embeddedChannel.readOutbound();
        assertEquals(Messages.MAGIC, byteBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(byteBuf.readBytes(16)));
        assertEquals(Messages.DELETE_DATA_REQUEST, byteBuf.readShort());
        assertEquals(100, byteBuf.readInt());

        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;

            int keyLength = byteBuf.readInt();
            String _key = byteBuf.readBytes(keyLength).toString(StandardCharsets.UTF_8);

            assertEquals(key, _key);
        }

        byteBuf.writeInt(Messages.DELIMITER);
        assertTrue(embeddedChannel.close().isSuccess());
    }

    @Test
    void deleteDataResponseTest() {
        EmbeddedChannel embeddedChannel = newEmbeddedChannel();

        byte[] id = new byte[16];
        new SecureRandom().nextBytes(id);

        ByteBuf byteBuf = Unpooled.buffer();
        byteBuf.writeInt(Messages.MAGIC);        // Write magic number
        byteBuf.writeBytes(id);                  // ID
        byteBuf.writeShort(Messages.DELETE_DATA_RESPONSE);
        byteBuf.writeInt(100);

        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;
            byteBuf.writeInt(key.length());
            ByteBufUtil.writeUtf8(byteBuf, key);
        }

        byteBuf.writeInt(Messages.DELIMITER);
        embeddedChannel.writeInbound(byteBuf);

        DeleteDataResponse response = embeddedChannel.readInbound();
        assertEquals(ByteBufUtil.hexDump(id), ByteBufUtil.hexDump(response.id())); // Verify ID
        assertEquals(100, response.keyValuePairs().size());

        int count = 0;
        for (KeyValuePair keyValuePair : response.keyValuePairs()) {
            assertEquals("Cat" + count, keyValuePair.key());
            count++;
        }

        embeddedChannel.writeOutbound(response);
        byteBuf = embeddedChannel.readOutbound();
        assertEquals(Messages.MAGIC, byteBuf.readInt());
        assertArrayEquals(id, ByteBufUtil.getBytes(byteBuf.readBytes(16)));
        assertEquals(Messages.DELETE_DATA_RESPONSE, byteBuf.readShort());
        assertEquals(100, byteBuf.readInt());

        for (int i = 0; i < 100; i++) {
            String key = "Cat" + i;

            int keyLength = byteBuf.readInt();
            String _key = byteBuf.readBytes(keyLength).toString(StandardCharsets.UTF_8);
            assertEquals(key, _key);
        }

        byteBuf.writeInt(Messages.DELIMITER);
        assertTrue(embeddedChannel.close().isSuccess());
    }

    private EmbeddedChannel newEmbeddedChannel() {
        return new EmbeddedChannel(new DelimiterBasedFrameDecoder(10_000_000,
                Unpooled.buffer().writeInt(Messages.DELIMITER)), new Encoder(), new Decoder()) {
            @Override
            protected SocketAddress remoteAddress0() {
                return new InetSocketAddress("127.0.0.1", 9110);
            }
        };
    }
}
