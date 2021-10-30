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
import com.shieldblaze.expressgateway.intercommunication.messages.request.MemberJoinRequest;
import com.shieldblaze.expressgateway.intercommunication.messages.request.MemberLeaveRequest;
import io.netty.channel.ChannelHandlerContext;
import io.netty.channel.SimpleChannelInboundHandler;

import static com.shieldblaze.expressgateway.intercommunication.Broadcaster.MEMBERS;

final class InboundHandler extends SimpleChannelInboundHandler<Message> {

    @Override
    protected void channelRead0(ChannelHandlerContext ctx, Message msg) {
        if (msg instanceof MemberJoinRequest) {
            join(ctx, (MemberJoinRequest) msg);
        } else if (msg instanceof MemberLeaveRequest) {
            leave(ctx, (MemberLeaveRequest) msg);
        }
    }

    private void join(ChannelHandlerContext ctx, MemberJoinRequest memberJoinRequest) {
        InboundHandler thisHandler = MEMBERS.get(memberJoinRequest.id());

        // Make sure member is not already added in cluster
        if (thisHandler != null) {
            ctx.writeAndFlush(new MemberAlreadyExistsError(memberJoinRequest.id()));
        } else {
            MEMBERS.put(memberJoinRequest.id(), this);
        }
    }

    private void leave(ChannelHandlerContext ctx, MemberLeaveRequest memberLeaveRequest) {
        InboundHandler thisHandler = MEMBERS.remove(memberLeaveRequest.id());

        if (thisHandler == null) {
            ctx.writeAndFlush(new MemberDoesNotExistsError(memberLeaveRequest.id()));
        }
    }
}
