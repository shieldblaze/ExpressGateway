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
import com.shieldblaze.expressgateway.intercommunication.messages.request.SimpleMessageRequest;
import com.shieldblaze.expressgateway.intercommunication.messages.request.UpsertDataRequest;
import com.shieldblaze.expressgateway.intercommunication.messages.response.DeleteDataResponse;
import com.shieldblaze.expressgateway.intercommunication.messages.response.MemberJoinResponse;
import com.shieldblaze.expressgateway.intercommunication.messages.response.MemberLeaveResponse;
import com.shieldblaze.expressgateway.intercommunication.messages.response.SimpleMessageResponse;
import com.shieldblaze.expressgateway.intercommunication.messages.response.UpsertDataResponse;
import io.netty.buffer.ByteBuf;
import io.netty.channel.ChannelFutureListener;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.ByteToMessageDecoder;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

/**
 * Converts inbound {@link ByteBuf} to {@link Messages} frames
 */
final class Decoder extends ByteToMessageDecoder {

    private static final Logger logger = LogManager.getLogger(Decoder.class);

    @Override
    protected void decode(ChannelHandlerContext ctx, ByteBuf in, List<Object> out) {
        try {
            int magicNumber = in.readInt();
            if (magicNumber != Messages.MAGIC().readInt()) {
                forceClose(ctx);
                return;
            }

            ByteBuf id = in.readBytes(16); // Read 128-bit UUID
            short type = in.readShort();

            if (type == Messages.JOIN_REQUEST().readShort()) {
                Message join = new MemberJoinRequest(id);
                out.add(join);
                return;
            } else if (type == Messages.JOIN_RESPONSE().readShort()) {
                Message join = new MemberJoinResponse(id);
                out.add(join);
                return;
            } else if (type == Messages.LEAVE_REQUEST().readShort()) {
                Message leave = new MemberLeaveRequest(id);
                out.add(leave);
                return;
            } else if (type == Messages.LEAVE_RESPONSE().readShort()) {
                Message leave = new MemberLeaveResponse(id);
                out.add(leave);
                return;
            } else if (type == Messages.UPSET_DATA_REQUEST().readShort()) {
                int count = in.readInt();
                List<KeyValuePair> keyValuePairs = new ArrayList<>(count);

                for (int i = 0; i < count; i++) {
                    int keyLength = in.readInt();
                    String key = in.readBytes(keyLength).toString(StandardCharsets.UTF_8);

                    int valueLength = in.readInt();
                    String value = in.readBytes(valueLength).toString(StandardCharsets.UTF_8);

                    keyValuePairs.add(new KeyValuePair(key, value));
                }

                Message message = new UpsertDataRequest(id, keyValuePairs);
                out.add(message);
                return;
            } else if (type == Messages.UPSET_DATA_RESPONSE().readShort()) {
                int count = in.readInt();
                List<KeyValuePair> keyValuePairs = new ArrayList<>(count);

                for (int i = 0; i < count; i++) {
                    int keyLength = in.readInt();
                    String key = in.readBytes(keyLength).toString(StandardCharsets.UTF_8);
                    keyValuePairs.add(new KeyValuePair(key));
                }

                Message message = new UpsertDataResponse(id, keyValuePairs);
                out.add(message);
                return;
            } else if (type == Messages.DELETE_DATA_REQUEST().readShort()) {
                int count = in.readInt();
                List<KeyValuePair> keyValuePairs = new ArrayList<>(count);

                for (int i = 0; i < count; i++) {
                    int keyLength = in.readInt();
                    String key = in.readBytes(keyLength).toString(StandardCharsets.UTF_8);
                    keyValuePairs.add(new KeyValuePair(key));
                }

                Message message = new DeleteDataRequest(id, keyValuePairs);
                out.add(message);
                return;
            } else if (type == Messages.DELETE_DATA_RESPONSE().readShort()) {
                int count = in.readInt();
                List<KeyValuePair> keyValuePairs = new ArrayList<>(count);

                for (int i = 0; i < count; i++) {
                    int keyLength = in.readInt();
                    String key = in.readBytes(keyLength).toString(StandardCharsets.UTF_8);
                    keyValuePairs.add(new KeyValuePair(key));
                }

                Message message = new DeleteDataResponse(id, keyValuePairs);
                out.add(message);
                return;
            } else if (type == Messages.SIMPLE_MESSAGE_REQUEST().readShort()) {
                int size = in.readInt();

                Message message = new SimpleMessageRequest(id, in.readBytes(size));
                out.add(message);
                return;
            } else if (type == Messages.SIMPLE_MESSAGE_RESPONSE().readShort()) {
                int size = in.readInt();

                Message message = new SimpleMessageResponse(id, in.readBytes(size));
                out.add(message);
                return;
            }
        } catch (Exception ex) {
            logger.error(ex);
        }
        forceClose(ctx);
    }

    private void forceClose(ChannelHandlerContext ctx) {
        ctx.writeAndFlush(ctx.alloc().buffer(2).writeShort(0xff))
                .addListener(ChannelFutureListener.CLOSE);

        logger.debug("Unknown/Corrupted frame received, Closing connection...");
    }
}
