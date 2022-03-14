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
import io.netty.buffer.ByteBufUtil;
import io.netty.channel.ChannelHandlerContext;
import io.netty.handler.codec.MessageToByteEncoder;
import io.netty.handler.codec.UnsupportedMessageTypeException;

/**
 * Converts outbound {@link Message} frames into {@link ByteBuf}
 */
final class Encoder extends MessageToByteEncoder<Message> {

    @Override
    protected void encode(ChannelHandlerContext ctx, Message msg, ByteBuf out) {
        out.writeBytes(Messages.MAGIC()); // Write magic
        out.writeBytes(msg.id());         // Write ID

        if (msg instanceof MemberJoinRequest) {
            out.writeBytes(Messages.JOIN_REQUEST());
        } else if (msg instanceof MemberJoinResponse) {
            out.writeBytes(Messages.JOIN_RESPONSE());
        } else if (msg instanceof MemberLeaveRequest) {
            out.writeBytes(Messages.LEAVE_REQUEST());
        } else if (msg instanceof MemberLeaveResponse) {
            out.writeBytes(Messages.LEAVE_RESPONSE());
        } else if (msg instanceof UpsertDataRequest request) {
            out.writeBytes(Messages.UPSET_DATA_REQUEST());
            out.writeInt(request.keyValuePairs().size());  // Write Size of all key value pair

            for (KeyValuePair pair : request.keyValuePairs()) {
                out.writeInt(pair.key().length());
                ByteBufUtil.writeUtf8(out, pair.key());

                out.writeInt(pair.value().length());
                ByteBufUtil.writeUtf8(out, pair.value());
            }
        } else if (msg instanceof UpsertDataResponse response) {
            out.writeBytes(Messages.UPSET_DATA_RESPONSE());
            out.writeInt(response.keyValuePairs().size()); // Write Size of all key value pair

            for (KeyValuePair pair : response.keyValuePairs()) {
                out.writeInt(pair.key().length());
                ByteBufUtil.writeUtf8(out, pair.key());
            }
        } else if (msg instanceof DeleteDataRequest request) {
            out.writeBytes(Messages.DELETE_DATA_REQUEST());
            out.writeInt(request.keyValuePairs().size()); // Write Size of all key value pair

            for (KeyValuePair pair : request.keyValuePairs()) {
                out.writeInt(pair.key().length());
                ByteBufUtil.writeUtf8(out, pair.key());
            }
        } else if (msg instanceof DeleteDataResponse response) {
            out.writeBytes(Messages.DELETE_DATA_RESPONSE());
            out.writeInt(response.keyValuePairs().size()); // Write Size of all key value pair

            for (KeyValuePair pair : response.keyValuePairs()) {
                out.writeInt(pair.key().length());
                ByteBufUtil.writeUtf8(out, pair.key());
            }
        } else if (msg instanceof SimpleMessageRequest request) {
            out.writeBytes(Messages.SIMPLE_MESSAGE_REQUEST());
            out.writeBytes(request.data());
        } else if (msg instanceof SimpleMessageResponse response) {
            out.writeBytes(Messages.SIMPLE_MESSAGE_RESPONSE());
            out.writeBytes(response.data());
        } else {
            throw new UnsupportedMessageTypeException("Unknown Message: " + msg);
        }
        out.writeBytes(Messages.DELIMITER());
    }
}
