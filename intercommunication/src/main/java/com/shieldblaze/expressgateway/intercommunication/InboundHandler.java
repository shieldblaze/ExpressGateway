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

import com.shieldblaze.expressgateway.intercommunication.messages.error.MemberAlreadyExistsError;
import com.shieldblaze.expressgateway.intercommunication.messages.error.MemberDoesNotExistsError;
import com.shieldblaze.expressgateway.intercommunication.messages.request.*;
import io.netty.buffer.Unpooled;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.SimpleChannelInboundHandler;

import java.util.List;

import static com.shieldblaze.expressgateway.intercommunication.Broadcaster.MEMBERS;

final class InboundHandler extends SimpleChannelInboundHandler<Message> {

    ChannelHandlerContext ctx;

    @Override
    public void channelActive(ChannelHandlerContext ctx) throws Exception {
        this.ctx = ctx;
    }

    @Override
    protected void channelRead0(ChannelHandlerContext ctx, Message msg) {
        if (msg instanceof MemberJoinRequest request) {
            join(ctx, request);
        } else if (msg instanceof MemberLeaveRequest request) {
            leave(ctx, request);
        } else if (msg instanceof UpsertDataRequest request) {
            upsert(ctx, request);
        } else if (msg instanceof DeleteDataRequest request) {

        } else if (msg instanceof SimpleMessageRequest request) {

        }
    }

    private void join(ChannelHandlerContext ctx, MemberJoinRequest request) {
        InboundHandler thisHandler = MEMBERS.get(request.id());

        // Make sure member is not already added in cluster
        if (thisHandler != null) {
            ctx.writeAndFlush(new MemberAlreadyExistsError(request.id()));
        } else {
            MEMBERS.put(request.id(), this);
        }
    }

    private void leave(ChannelHandlerContext ctx, MemberLeaveRequest request) {
        InboundHandler thisHandler = MEMBERS.remove(request.id());

        if (thisHandler == null) {
            ctx.writeAndFlush(new MemberDoesNotExistsError(request.id()));
        }
    }

    private void upsert(ChannelHandlerContext ctx, UpsertDataRequest request) {
        InboundHandler thisHandler = MEMBERS.get(request.id());

        if (thisHandler == null) {
            ctx.writeAndFlush(new MemberDoesNotExistsError(request.id()));
        } else {

        }
    }
}
